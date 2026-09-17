# V.92 clauses 8.1 and 8.2: full Phase 1 and the analogue modem's short Phase 1 signals (implementation digest)

Source: ITU-T V.92 (11/2000). Clause 8 (introduction), 8.1, and 8.2 to 8.2.5 (Tables 2 to 5) are on
PDF pages 13 to 16 (printed pages 6 to 9). All four pages were rendered and read as images. Every bit
pattern, field position, value and timing below comes from those rendered pages, not from
`docs/specs/text`. The extracted text for these tables is lossy: it drops or moves characters, and its
Table 6 even swaps the µ-law and A-law columns. Do not check this file against the extracted text.

Other material used, also read from rendered pages:

| Document | PDF pages | Used for |
|---|---|---|
| V.92 (11/2000) | 16 (8.3.1, Table 6); 21 to 22 (8.3.2 to 8.3.5, Tables 11 to 14); 23 (8.3.6 QTS); 46 to 50 (9.1, 9.2, Figures 3 to 8, 9.2.5); 66 (9.10.2.1) | Signals the analogue modem must **receive**, and the procedures that use the 8.2 signals |
| V.8 (11/2000) | 9 to 18 (clauses 5 to 8, Tables 1 to 8, Figure 1) | "V.8-type formatting", the sync patterns, ANSam, CM, JM, CJ, CI, Te |
| V.8 bis (11/2000) | 13 to 21 (clause 7, clause 8 up to 8.3.3, Tables 1 to 5-1, Figures 1 to 8) | QC2a/QCA2a frame structure, identification field, FCS, CRe |
| V.21 (1988) | 3 to 4 | V.21(L) and V.21(H) frequencies and tolerances |
| V.25 (10/96) | 6 | ANS, phase reversals, calling tone |
| V.90 (09/98) | 11 to 12 (Table 1, Ucodes); 20 (8.1); 36 to 37 (9.1) | Ucode values of U_QTS; full Phase 1 procedure and power level |
| V.34 (02/98) | 31 (10.1.1); text of 11.1 and 11.2 | Phase 1 power level; where "V.34 Phase 2" starts |

Scope boundary. The procedures in 9.2 and the digital modem's signals in 8.3 belong to other digests.
They are summarised here (sections 9 and 10) only because the analogue modem has to decode the digital
modem's signals and has to know when each of its own signals is sent.

Conventions:

- **SHALL** = mandatory, **MAY** = optional, **SHOULD** = recommended. These are the Recommendation's own
  words. **[DERIVED]** marks something worked out here that the text does not say directly. **[AMBIGUOUS]**
  marks a gap or a contradiction in the text.
- Bit ranges are written `a:b` as in the tables. Bit 0 is sent first in time.
- Requirements are numbered `P1-nn`, each with its clause.
- "T" in V.92 figures means one PCM symbol = 1/8000 s = 125 µs.

---

## 1. Summary

1. **8.1**: full Phase 1 adds no new signals. Every signal is the V.25, V.8 or V.8 bis one. The procedure is
   the V.90 Phase 1 one (9.1/V.92).
2. **Clause 8 introduction**: a bit-order rule that applies to Tables 2 to 5 and to many later tables (section 2).
3. **8.2**: four new FSK signals from the analogue modem, plus one tone:

| Signal | Clause / table | Sent by (the analogue modem as...) | Channel | Framing | Length on line | Carries |
|---|---|---|---|---|---|---|
| **QC1a** | 8.2.1 / Table 2 | caller, after ANSam (V.8 route) | **V.21(L)** | V.8-type 10-bit frames | 60 bits = 200 ms, **sent once, then CM immediately** | analogue, QC, P (LAPM), U_QTS |
| **QC2a** | 8.2.2 / Table 3 | caller, after CRe (V.8 bis route) | **V.21(H)** | V.8 bis HDLC-like message | ≥ 86 bits ≈ 287 ms (section 6.5) | message type, V.8 bis revision, U_QTS, P, QC, analogue |
| **QCA1a** | 8.2.3 / Table 4 | answerer, after QC1d (or QC1a) | **V.21(H)** | V.8-type 10-bit frames | 70 bits ≈ 233 ms, **sent once** | analogue, QCA, P, U_QTS |
| **QCA2a** | 8.2.4 / Table 5 | answerer, after QC2d (or QC2a) | **V.21(L)** | V.8 bis HDLC-like message | ≥ 86 bits ≈ 287 ms | as QC2a, with the QCA flag set |
| **TONEq** | 8.2.5 | caller or answerer (acknowledges ANSpcm or ANSam) | tone | none | not fixed (≥ 50 ms in 9.2.1.3) | nothing: a 980 Hz tone |

QC = "quick connect" request. QCA = its acknowledgement. The suffix "a" means analogue-modem signal. "1"
means the V.8 route (ANSam answer). "2" means the V.8 bis route (CRe answer). The Recommendation never
expands "QC", "QCA" or "TONEq". These readings come from how the signals are used.

---

## 2. The clause 8 bit-order rule (clause 8, PDF p.13)

P1-01 (clause 8). Every PCM codeword named in a training sequence is a **universal code (Ucode)** as in
Table 1/V.90.

P1-02 (clause 8). In Tables 2 to 5 (and 11 to 24, 27, 30 to 33), unless a table says otherwise:

- a value written as a **bit pattern** is sent **leftmost bit first**;
- a value written as an **integer** is sent **least significant bit first**.

Consequences for 8.2:

- `1111111111`, `0101010101`, `W0XYZ1`, `000PW0XYZ1`, `1011`, `VVVV` and `WXYZ` are all patterns. The
  leftmost character goes first. In `WXYZ`, **W is first in time** and Z is last.
- Table 2 lists the U_QTS codes as the patterns `0000` to `1111`, so W is first there too. Read them as
  patterns. Do not reverse them.
- Across the whole signal, bit `n` goes before bit `n+1`.

---

## 3. Clause 8.1: full Phase 1

### 3.1 What 8.1 says

P1-03 (8.1). All full Phase 1 signals and sequences are those defined in **V.25, V.8 or V.8 bis**. V.92 adds
nothing and changes nothing.

P1-04 (9.1/V.92, p.46). The full Phase 1 **procedure** is the one in V.90 Phase 1 (9.1/V.90). The NOTE in
9.1/V.92 says V.8 has no way to indicate V.92 specifically, so the V.92 decision is deferred. It is made in
Phase 2 through INFO0 bits: INFO0d bit 27 and INFO0a bit 26 (9.3/V.92; see `spec-phase2-signals.md`).

### 3.2 What the referenced texts require

**V.90 8.1 (p.20).** V.8 is used in Phase 1, and V.8 bis optionally. Every Phase 1 signal is defined in
V.25 or V.8. **All of them SHALL be sent at the nominal transmit power level.** V.34 10.1.1 (p.31) says the
same for V.34 Phase 1. "Nominal transmit power" is the user-configured reference power (3.4/V.92,
3.4/V.90).

**V.90 9.1 (pp.36 to 37), the full Phase 1 procedure:**

- 9.1.1: the call menu shows V.90 capability through the PCM category. In V.8 (2000) wording, this means
  `modn0` b5 = 1 plus the "PCM modem availability" octet `pcm0` (see below). A modem that shows V.90
  capability SHALL also send its PSTN access type. PCM operation needs one analogue and one digital modem.
  If both modems are on analogue access, or the categories do not show an analogue/digital pair, both SHALL
  continue under V.8 as if PCM had not been offered. If both modems are digitally connected and both offer
  both roles, **the caller becomes the analogue modem** and the answerer the digital modem.
- 9.1.2.1, caller: listen for ANS or ANSam, and send CI, CT, CNG or nothing. On ANSam, stay silent for Te,
  then send CM with the PCM bits and listen for JM. After at least **2 identical JM** sequences, finish the
  current CM octet and send **CJ**. Then **silence for 75 ± 5 ms**, then Phase 2 (Figure 3/V.90).
- 9.1.2.2: on ANS instead of ANSam, follow Annex A/V.32 bis, T.30 or another suitable Recommendation.
- 9.1.3.1, answerer: stay **silent for ≥ 200 ms**, then send **ANSam with phase reversals**, and listen for CM
  and any other calling-modem responses.
- 9.1.3.2: after **2 identical CM** sequences showing V.90, send JM and listen for CJ. After **all 3 CJ
  octets**, **silence for 75 ± 5 ms**, then Phase 2.
- 9.1.3.3 and 9.1.3.4: other responses go to their own Recommendation. If nothing suitable arrives within
  the ANSam period, send 75 ± 5 ms of silence, then follow Annex A/V.32 bis, T.30 and so on.

**V.8 (11/2000), items the analogue modem uses in full Phase 1:**

| Item | Requirement (V.8 clause) |
|---|---|
| Frame format (5) | CI, CM and JM each repeat one sequence: **10 ONEs, then 10 sync bits, then octets**. Each octet is framed by a **start bit (0)** and a **stop bit (1)**. Order on the line: `start b0 b1 b2 b3 b4 b5 b6 b7 stop`. b0 is the LSB of the octet. |
| Sync patterns (Table 1, p.9) | CI: `0000000001`. CM and JM: `0000001111`. **`0101010101`: "Defined in ITU-T V.92"** (this is the QC/QCA sync). Each is written in transmission order. As a framed octet, `0000001111` = 0xE0 and `0101010101` = **0x55**. |
| Category octet (5.1) | b0 to b3 = category tag (b0 = LSB), **b4 = 0**, b5 to b7 = option bits. |
| Extension octet (5.2) | `b0 b1 b2 0 1 0 b6 b7`: b3 = 0, b4 = 1, b5 = 0. |
| Category tags (Table 2) | Call function b0..b3 = `1000`. Modulation modes `1010`. Protocols `0101`. PSTN access `1011`. Non-standard `1111`. PCM modem availability `1110`. T.66 `0111`. (Each written b0 first.) |
| callf0 (Table 3) | b5 b6 b7 = `011` for data. |
| modn0 (Table 4) | b5 = 1 when the PCM category is present. b6 = V.34 duplex. b7 = V.34 half-duplex. modn1 and modn2 carry the older modulations. |
| pcm0 (Table 5) | **b5 = V.90 or V.92 analogue modem.** b6 = V.90 or V.92 digital modem. b7 = V.91. If b5 or b6 is set, the V.34 bit SHALL also be set, the PSTN access category SHALL be present, and modn0 b5 SHALL be 1. |
| prot0 (Table 6) | b5 b6 b7 = `100` asks for LAPM. If both ends indicate LAPM they **may be required** to skip ODP/ADP (7.3 and 7.4/V.8 cite 9.3.1/V.92). Some V.8 implementations still need ODP/ADP. |
| access0 (Table 7) | b5 = caller is on a cellular connection. b6 = answerer is on a cellular connection. **b7 = 1: this DCE is on a digital network connection; b7 = 0: analogue.** NOTE 2: an analogue V.92 modem may sit on a digital network connection. |
| ANSam (7.2) | **2100 ± 1 Hz**, **15 ± 0.1 Hz** sinusoidal AM, envelope between **(0.8 ± 0.01)** and **(1.2 ± 0.01)** × the mean amplitude, **phase reversals every 450 ± 25 ms** (these may be left out only when echo-canceller disabling is not needed; V.90 requires them). Out-of-band power (outside 2100 ± 200 Hz) at least **24 dB** below in-band power. Power per V.2. A caller SHALL NOT send CM before it has detected ANSam. |
| CM, JM and CJ (3.4 to 3.6, 7.3, 7.4) | CM is on **V.21(L)**, JM on **V.21(H)**, both at 300 bit/s. **CJ = three all-zero octets** with start and stop bits, on V.21(L). JM SHALL only be sent after at least 2 identical CMs. |
| Te (8.1.1) | Silence before CM, starting at the end of the call signal (or at ANSam detection if there is none). **Te ≥ 0.5 s**, and **≥ 1 s** if V.25 echo-canceller disabling is wanted. |
| ANSam length (8.2.2) | If no CM or sigC stops it, ANSam lasts **5 ± 1 s**. |
| Answerer silence (8.2) | **≥ 0.2 s** of silence after connecting to the line. |
| CI (7.1) | Sent with an ON/OFF cadence. ON lasts at least 3 CI sequences and at most 2 s. OFF lasts 0.4 s to 2 s. Optional. |

**V.25 (p.6).** ANS is an uninterrupted **2100 ± 15 Hz** tone lasting **3.3 ± 0.7 s** (unless cut short).
Phase reversals are **180° every 425 to 475 ms**. Each reversal SHALL land within 180 ± 10° in 1 ms, and the
amplitude SHALL NOT stay more than 3 dB low for more than 400 µs. The calling tone is **1300 ± 15 Hz**, ON
0.5 to 0.7 s, OFF 1.5 to 2.0 s. Power levels follow V.2.

**V.8 bis** is optional in full Phase 1 (V.90 8.1). The parts that short Phase 1 needs are in section 6.

---

## 4. Clause 8.2: common rules for the analogue modem's short Phase 1 signals

### 4.1 Which signal goes with which start-up (8.2, p.14)

P1-05 (8.2). **QC1a and QCA1a** are for calls **started under V.8** (the answerer sent ANSam). **QC2a and
QCA2a** are for calls **started under V.8 bis** (the answerer sent CRe).

### 4.2 Modulation

P1-06 (8.2). Short Phase 1 information bits are sent at **300 bit/s**, FSK-modulating either **V.21(L)**
(the low-band channel of V.21) or **V.21(H)** (the high-band channel).

V.21 (p.3 to p.4) channel parameters:

| Channel | Mean frequency | Binary 1 (mark, F_Z) | Binary 0 (space, F_A) |
|---|---|---|---|
| V.21(L) = channel No. 1 | 1080 Hz | **980 Hz** | **1180 Hz** |
| V.21(H) = channel No. 2 | 1750 Hz | **1650 Hz** | **1850 Hz** |

- The deviation is ±100 Hz. The higher frequency is binary 0.
- Transmit frequencies SHALL be within **±6 Hz** of nominal. Receivers must allow **±12 Hz** (V.21 §3).
- The modulation rate equals the data rate: one bit = 1/300 s = 3.333 ms = 26.667 samples at 8 kHz.
- V.8 bis messages (QC2a, QCA2a) also carry V.8 bis's tighter tolerance: **F_A and F_Z within ±0.01%**
  (7.2/V.8 bis).
- V.21 §6: output into the line SHALL NOT exceed 1 mW. The level at an international circuit input SHALL
  NOT exceed −13 dBm0 (V.2).

Channel use (8.2.1 to 8.2.4, rendered):

| Signal | Channel | Same channel as |
|---|---|---|
| QC1a | V.21(L) | CM, CI, CJ (caller → answerer) |
| QCA1a | V.21(H) | JM (answerer → caller) |
| QC2a | **V.21(H)** | V.8 bis messages from the **responding** station (the caller answers the answerer's CRe) |
| QCA2a | **V.21(L)** | V.8 bis messages from the **initiating** station (the answerer, which sent CRe) |

[DERIVED] The V.8 bis signals therefore run in the opposite direction to the V.8 ones. This follows the
V.8 bis rule (7.2/V.8 bis): the initiating station uses V.21(L) and the responding station uses V.21(H).
CRe makes the **answering** modem the initiating station.

### 4.3 Transmit level

[AMBIGUOUS] V.92 8.2 gives **no transmit level** for QC1a, QC2a, QCA1a, QCA2a or TONEq, and no tolerance for
TONEq. Nearby rules:

- V.90 8.1 and V.34 10.1.1: every Phase 1 signal at **nominal transmit power**.
- V.8 bis 7.2.1: message power per national regulations (V.2 as a guide).
- V.8 bis 7.1.4: CRe and MRe must be 12 to 15 dB below nominal. That rule does **not** apply to messages.

**Recommendation:** send all 8.2 signals at the nominal transmit power, the same as CM and JM.

### 4.4 Two framings

- **V.8-type** (QC1a, QCA1a): section 5.
- **V.8 bis type** (QC2a, QCA2a): section 6.

---

## 5. V.8-type framed signals: QC1a and QCA1a

### 5.1 Frame skeleton shared by QC1a, QCA1a, QC1d and QCA1d

P1-07 (8.2.1, 8.2.3). The signal is a string of **10-bit frames in V.8 format**:

```
bits  0:9   1111111111   ten ONEs   (V.8 preamble)
bits 10:19  0101010101   sync       (V.8 Table 1 "Defined in ITU-T V.92")
bits 20:29  0 D A P f f f f f 1     information frame (start bit, 8 bits, stop bit)
bits 30:39  1111111111   ten ONEs
bits 40:49  0101010101   sync again
bits 50:59  copy of bits 20:29
[bits 60:69 1111111111   ten ONEs  -- QCA1a / QCA1d only]
```

Bit meanings in the information frame (bit 20 = start):

| Bit | QC1a (Table 2) | QCA1a (Table 4) | Meaning |
|---|---|---|---|
| 20 | `0` | `0` | start bit |
| 21 | `0` | `0` | **0 = analogue modem** (1 = digital modem, in QC1d/QCA1d) |
| 22 | `0` | `1` | **0 = QC**, 1 = QCA |
| 23 | `P` | `P` | 1 = asks for LAPM per V.42 (see 9.2.5) |
| 24:29 | `W0XYZ1` | `W0XYZ1` | U_QTS code WXYZ, with a fixed 0 at bit 25 and the stop bit (1) at bit 29 |

[DERIVED] Mapping onto a V.8 octet: bits 21 to 28 are `b0` to `b7`, bit 20 is the start bit and bit 29 is the
stop bit. So:

- b0 = D (bit 21), b1 = A (bit 22), b2 = P (bit 23), b3 = W (bit 24)
- **b4 = 0** (bit 25). This is the V.8 "category octet" marker that prevents flag simulation.
- b5 = X (bit 26), b6 = Y (bit 27), b7 = Z (bit 28)

Octet value, LSB first on the line:

```
octet = D | A<<1 | P<<2 | W<<3 | X<<5 | Y<<6 | Z<<7          (QC1a: D=0,A=0; QCA1a: D=0,A=1)
```

The signal can then be produced by an ordinary asynchronous V.8 framer, octet by octet. Emit the preamble
of 10 marks, then the framed octets **0x55** (sync) and **info**, then 10 marks, then **0x55** and
**info** again. For QCA1a, add 10 more marks. Emit no gaps and no extra stop bits.

[DERIVED] **A QC/QCA information frame can never look like the QC/QCA sync.** The sync needs a 1 at frame
bit 5 (bit 25), and bit 25 is always 0. **It can look like a V.8 sync, though**:

- QC1a with P = 0 and WXYZ = `0000` gives the info frame `0000000001`. That is the **CI sync**.
- QC1a with P = 0 and WXYZ = `0111` gives `0000001111`. That is the **CM/JM sync**.

A receiver must accept a sync only when it follows the run of ten (or more) ONEs directly. A receiver
that looks for sync patterns anywhere in the stream will misread these frames.

### 5.2 QC1a (8.2.1, Table 2, p.14)

P1-08 (8.2.1). QC1a is sent on **V.21(L)**, formatted as in Table 2.

P1-09 (8.2.1). QC1a is **sent once** and is **followed immediately by CM**. There is no silence and no
extra marks between bit 59 of QC1a and the first of CM's ten ONEs.

Full layout (rendered Table 2):

| Bit position | Content | Definition |
|---|---|---|
| 0:9 | `1111111111` | Ten ONEs |
| 10:19 | `0101010101` | Synchronization sequence |
| 20 | `0` | Start bit |
| 21 | `0` | Indication for analogue modem |
| 22 | `0` | Indication for QC |
| 23 | `P` | Set to 1 to ask for LAPM per V.42 (see 9.2.5) |
| 24:29 | `W0XYZ1` | WXYZ = U_QTS, the Ucode of the PCM codeword to use for QTS (code table below) |
| 30:39 | `1111111111` | Ten ONEs |
| 40:49 | `0101010101` | Bits 10:19 repeated |
| 50:59 | `000PW0XYZ1` | Bits 20:29 repeated |

Total 60 bits = **200 ms** at 300 bit/s.

**U_QTS code table** (Table 2). Codes are bit patterns, W first in time. The linear values are from Table
1/V.90, rendered pp.11 to 12. They are shown so the analogue receiver can predict the QTS amplitude.

| WXYZ | U_QTS (Ucode) | µ-law PCM | µ-law linear | A-law PCM | A-law linear |
|---|---|---|---|---|---|
| `0000` | 61 | C2 | 1756 | E8 | 1888 |
| `0001` | 62 | C1 | 1820 | EB | 1952 |
| `0010` | 63 | C0 | 1884 | EA | 2016 |
| `0011` | 66 | BD | 2236 | 97 | 2368 |
| `0100` | 67 | BC | 2364 | 96 | 2496 |
| `0101` | 70 | B9 | 2748 | 93 | 2880 |
| `0110` | 71 | B8 | 2876 | 92 | 3008 |
| `0111` | 74 | B5 | 3260 | 9F | 3392 |
| `1000` | 75 | B4 | 3388 | 9E | 3520 |
| `1001` | 78 | B1 | 3772 | 9B | 3904 |
| `1010` | 79 | B0 | 3900 | 9A | 4032 |
| `1011` | 82 | AD | 4604 | 87 | 4736 |
| `1100` | 83 | AC | 4860 | 86 | 4992 |
| `1101` | 86 | A9 | 5628 | 83 | 5760 |
| `1110` | 87 | A8 | 5884 | 82 | 6016 |
| `1111` | — | — | — | — | **Cleardown from on-hold state** (not a Ucode) |

(The V.90 linear scale runs to 32124 for µ-law and 32256 for A-law. Every value above was checked
against the G.711 segment formulas. [DERIVED])

P1-10 (8.2.1 with 9.10.2.1). WXYZ = `1111` means **"cleardown from on-hold state"**. An on-hold modem that
receives a QC with U_QTS = `1111` SHALL disconnect (9.10.2.1/V.92, p.66; see `spec-modem-on-hold.md`). So
an analogue modem that wants to **clear down a held call** sends QC1a (or QC2a) with WXYZ = `1111`, not a
normal QC. A normal quick-connect SHALL use one of the 15 Ucode entries.

[AMBIGUOUS] V.92 gives **no rule for choosing U_QTS**. The analogue modem picks the QTS level the digital
modem will send (section 10.2). Presumably it picks a level its receiver handles well. The range is Ucode
61 (µ-law linear 1756) to 87 (5884).

Worked examples [DERIVED], shown in 10-bit groups in transmission order:

```
QC1a P=1 WXYZ=0101 (U_QTS=70):
 1111111111 0101010101 0001001011 1111111111 0101010101 0001001011     info octet 0xA4
QC1a P=0 WXYZ=0000 (U_QTS=61):
 1111111111 0101010101 0000000001 1111111111 0101010101 0000000001     info octet 0x00
QC1a P=1 WXYZ=1111 (cleardown from on-hold):
 1111111111 0101010101 0001101111 1111111111 0101010101 0001101111     info octet 0xEC
```

### 5.3 QCA1a (8.2.3, Table 4, p.15)

P1-11 (8.2.3). QCA1a is sent on **V.21(H)**, formatted as in Table 4, **once**.

| Bit position | Content | Definition |
|---|---|---|
| 0:9 | `1111111111` | Ten ONEs |
| 10:19 | `0101010101` | Synchronization sequence |
| 20 | `0` | Start bit |
| 21 | `0` | Indication for analogue modem |
| 22 | `1` | Indication for QCA |
| 23 | `P` | Set to 1 to ask for LAPM per V.42 (see 9.2.5) |
| 24:29 | `W0XYZ1` | U_QTS: WXYZ from Table 2 |
| 30:39 | `1111111111` | Ten ONEs |
| 40:49 | `0101010101` | Bits 10:19 repeated |
| 50:59 | `001PW0XYZ1` | Bits 20:29 repeated |
| 60:69 | `1111111111` | Ten ONEs |

Total 70 bits = **233.3 ms**. It ends with ten ONEs. QC1a does not, because CM follows it straight away.
In 9.2.3.1 the answerer **follows QCA1a with silence**.

Octet: `0x02 | P<<2 | W<<3 | X<<5 | Y<<6 | Z<<7`.

Meaning of U_QTS in QCA1a:

- When QCA1a answers a **QC1d** (digital caller, 9.2.3.1), U_QTS tells the digital caller which Ucode to use
  for the QTS it sends next.
- When QCA1a answers a **QC1a** (both modems analogue, Figure 7), no QTS follows and the field has no use.
  [AMBIGUOUS] V.92 does not say what to send there. **Recommendation:** send a valid Ucode code, never
  `1111`.

Worked examples [DERIVED]:

```
QCA1a P=1 WXYZ=0101:
 1111111111 0101010101 0011001011 1111111111 0101010101 0011001011 1111111111   info octet 0xA6
QCA1a P=0 WXYZ=0000:
 1111111111 0101010101 0010000001 1111111111 0101010101 0010000001 1111111111   info octet 0x02
```

---

## 6. V.8 bis-type signals: QC2a and QCA2a

### 6.1 What 8.2.2 and 8.2.4 require

P1-12 (8.2.2). QC2a is sent on **V.21(H)**. It uses the **signal structure of clause 7/V.8 bis** and the
**information field structure of clause 8/V.8 bis**. The analogue modem SHALL encode the identification field
as in **Table 3**.

P1-13 (8.2.4). QCA2a is sent on **V.21(L)**, with the same V.8 bis clause 7 and clause 8 structure. The
analogue modem SHALL encode the identification field as in **Table 5**.

P1-14 (Table 3 and Table 5 NOTE). VVVV is the V.8 bis revision number. At publication it was `0100`. **The
receiving modem SHALL ignore this field.**

### 6.2 Identification field (Tables 3 and 5, rendered pp.15 and 16)

| Bit position | QC2a content (Table 3) | QCA2a content (Table 5) | Definition |
|---|---|---|---|
| 0:3 | `1011` | `1011` | Message type |
| 4:7 | `VVVV` | `VVVV` | V.8 bis revision number (NOTE) |
| 8:11 | `WXYZ` | `WXYZ` | U_QTS from Table 2 |
| 12 | `0` | `0` | Reserved for ITU |
| 13 | `P` | `P` | Set to 1 to ask for LAPM per V.42 (see 9.2.5) |
| 14 | `0` | `1` | **0 = QC identifier; 1 = QCA identifier** |
| 15 | `0` | `0` | Analogue modem (the digital modem's QC2d/QCA2d have 1 here) |

### 6.3 How the 16 bits sit in V.8 bis octets [DERIVED, cross-checked]

V.8 bis 7.2.2: octets go out in ascending order, and **bit 1 of each octet goes first**. For a field inside
one octet, the lowest-numbered bit is the LSB. So V.92 bit k (sent k-th) is **V.8 bis octet ⌊k/8⌋+1, bit
(k mod 8)+1**, and it has weight 2^(k mod 8) in that octet.

Two checks show the mapping is right:

- **Message type.** V.92 writes `1011` in transmission order, so V.8 bis bits 1, 2, 3, 4 = 1, 0, 1, 1. Written
  as bits 4, 3, 2, 1 that is `1101`, which is exactly the code **Table 3/V.8 bis lists as "Defined in ITU-T
  V.92"** (rendered p.20). As a nibble value it is **0xD, not 0xB**.
- **Revision.** V.92 writes `0100` in transmission order, so V.8 bis bits 5, 6, 7, 8 = 0, 1, 0, 0. Written as
  bits 8, 7, 6, 5 that is `0010`, which is **"Revision 2"** in Table 4/V.8 bis (rendered p.21). The V.8 bis
  text also says its current revision is 2.

Octets:

```
octet 1 = 0x0D | (rev << 4)          rev = 2  ->  0x2D
octet 2 = W | X<<1 | Y<<2 | Z<<3 | P<<5 | Q<<6 | D<<7
          (bit 12 -> 0x10 is reserved 0; Q = 0 for QC2a, 1 for QCA2a; D = 0 for analogue)
```

8.3.2/V.8 bis: the tree-structure coding rules do **not** apply to the message type or revision fields.
[AMBIGUOUS] For the rest of the identification field, V.92 fixes 16 bits and says nothing about the V.8 bis
standard information field (S) or the non-standard field (NS). Under the V.8 bis delimiting rule (8.2.3),
bit 8 of an NPar(1) octet set to 0 would mean "another octet follows". Here octet 2's bit 8 is the
analogue/digital flag, which is 0 for an analogue modem. So the V.8 bis tree rules cannot be applied to
octet 2. **Recommendation:**

- transmit **exactly two** information-field octets;
- in the receiver, check the message-type nibble (0xD) first; if it matches, decode the fixed 16-bit V.92
  layout and ignore any further octets.

### 6.4 Message framing (V.8 bis clause 7, rendered pp.14 to 17)

P1-15 (7.2.4/V.8 bis). Every message **SHALL start with 100 ms ± 2%** of continuous V.21 **mark** (30 bits
at 300 bit/s).

P1-16 (7.2.5/V.8 bis). The message **SHALL start and end with HDLC flags `01111110`**:

- **at least 2 and at most 5** opening flags;
- **at least 1 and at most 3** flags after the FCS.

P1-17 (7.2.3, Figure 4/V.8 bis). Order on the line: flags (2 to 5), information field, FCS first octet, FCS
second octet, flags (1 to 3).

P1-18 (7.2.6/V.8 bis). The information field SHALL be a whole number of octets, coded per clause 8/V.8 bis.

P1-19 (7.2.7/V.8 bis). The **FCS is 16 bits**, ISO/IEC 3309 style: generator **x^16 + x^12 + x^5 + 1**. The
register is preset to all ONEs, and the ones' complement of the remainder is sent. Coverage: every bit
between the last opening flag and the FCS, **excluding stuffed zeros**.

- Receiver check: with the register preset to ones, the remainder over the data plus FCS is
  `0001110100001111` (x^15 to x^0) when there are no errors.
- FCS bit order (Figure 3/V.8 bis): bit 1 of the first FCS octet is the **MSB** (x^15 term) and bit 8 of the
  second octet is the LSB. Since bit 1 goes first, the x^15 coefficient goes first on the line.
- [DERIVED] This is the usual HDLC/X.25 CRC:
  - reflected polynomial 0x8408, init 0xFFFF, output XOR 0xFFFF, data processed LSB first;
  - send `fcs & 0xFF` first, then `fcs >> 8`, each LSB first;
  - receiver residue in reflected form = **0xF0B8** (the bit-reverse of 0x1D0F above);
  - check value for "123456789" = 0x906E.
  The HDLC FCS already in `crates/ec` should be reusable.

P1-20 (7.2.8/V.8 bis). **Zero-bit insertion.** Between the flags (information field and FCS), the sender
SHALL insert a 0 after every five consecutive 1s. The receiver SHALL delete any 0 that follows five
consecutive 1s.

P1-21 (7.2.9/V.8 bis). A frame is **invalid** if any of these is true:

- it is not bounded by flags as in 7.2.5;
- it has **fewer than 3 octets** between the flags;
- it is not a whole number of octets (before stuffing or after unstuffing);
- the FCS is wrong.

(Invalid-frame handling in 9.8/V.8 bis is a NAK(1). Short Phase 1 does not say whether that applies. See
section 12.)

Frequency tolerance (7.2/V.8 bis): **F_A and F_Z within ±0.01%**. Power (7.2.1/V.8 bis): national rules,
with V.2 as a guide (see section 4.3).

### 6.5 Size and worked examples [DERIVED]

With 2 opening flags, 1 closing flag and no stuffing: 30 + 16 + 16 + 16 + 8 = **86 bits = 286.7 ms**. With 5
opening and 3 closing flags: 134 bits + stuffing ≈ 450 ms. In QC2a and QCA2a, stuffing can only come from the
FCS octets: no run of five 1s fits in the two information octets, and octet 2 ends with the analogue flag, 0.
In the digital modem's QC2d and QCA2d that last bit is 1, so a run can cross into the FCS.

```
QC2a P=1 WXYZ=0101 VVVV=0100: ident bits 1011 0100 0101 0 1 0 0
  octets 0x2D 0x2A, FCS = 0x1254, sent as 0x54 then 0x12
  line (86 bits, 2+1 flags):
  111111111111111111111111111111 01111110 01111110 1011010001010100 0010101001001000 01111110
QC2a P=0 WXYZ=0000: octets 0x2D 0x00, FCS 0x9C0C (sent 0x0C, 0x9C)
QC2a P=1 WXYZ=1111 (cleardown from on-hold): octets 0x2D 0x2F, FCS 0x45F9 (sent 0xF9, 0x45),
  one zero is stuffed inside the FCS -> 87 bits
QCA2a P=1 WXYZ=0101: octets 0x2D 0x6A, FCS 0x5050 (sent 0x50, 0x50)
QCA2a P=0 WXYZ=0000: octets 0x2D 0x40, FCS 0xDE08 (sent 0x08, 0xDE)
```

(Here "FCS" is the value after complementing, in the reflected-register convention. The two octets are
sent in the order shown, each LSB first. The residue over data plus FCS is 0xF0B8 in every example.)

### 6.6 CRe, which triggers QC2a (V.8 bis 7.1, rendered pp.13 to 14)

CRe (the capabilities request sent by an automatic answering station) has two segments:

- **Segment 1**: dual tone **1375 + 2002 Hz** (the "initiating" pair), **400 ms** nominal. For MRe and CRe it
  **may be shortened to 285 ms** for compatibility with non-V.8 bis modems.
- **Segment 2**: single tone **400 Hz**, **100 ms** nominal.
- Tolerances: frequency **±250 ppm**, segment durations **±2%**.
- Power: **12 to 15 dB below** nominal transmit power.

The responding pair (1529 + 2225 Hz) belongs to MRd, CRd and ESr and is not used here.

P1-22 (9.2.1.2, Figure 5). The analogue caller starts QC2a once it has detected the **first 50 ms** of CRe.
Figure 5 marks this as "≥ 50 ms" from the start of CRe to the start of QC2a, so QC2a normally **overlaps the
rest of CRe**. The QC2a receiver must therefore work while its own CRe is still being sent (2002 Hz is
close to the V.21(H) band). [DERIVED]

---

## 7. TONEq (8.2.5, p.16)

P1-23 (8.2.5). **TONEq is a 980 Hz tone.** No level, frequency tolerance, phase or duration is given in 8.2.5.

- Duration comes from the procedures:
  - **at least 50 ms** in 9.2.1.3;
  - otherwise it lasts until the far end's ANSpcm or ANSam stops (9.2.1.3, 9.2.1.4, 9.2.3.3).
- [DERIVED] **980 Hz is exactly the V.21(L) mark frequency.** On the line, TONEq looks the same as a V.21(L)
  transmitter holding a steady mark.
- Recommended level: nominal transmit power (section 4.3).
- Recommended frequency accuracy: at least as tight as V.21 (±6 Hz). A soft modem produces 980 Hz exactly.

Who sends TONEq, and who listens for it:

| Clause | Sender | Trigger | End |
|---|---|---|---|
| 9.2.1.3 | analogue **caller** | ANSpcm detected for 1 s (or, **MAY**, as soon as ANSpcm is detected if ANSam was already detected for 1 s in 9.2.1.1) | ≥ 50 ms, and until ANSpcm is no longer detected; then 75 ± 5 ms of silence, then Phase 2 |
| 9.2.1.4 (from 9.2.1.1) | analogue caller, both modems analogue | QCA1a detected, then ANSam detected | until ANSam is no longer detected; then 75 ± 5 ms of silence, then **V.34 Phase 2** |
| 9.2.1.4 (from 9.2.1.2) | analogue caller, both modems analogue | QCA2a detected, then ANSam detected **for 1 s** | as above |
| 9.2.3.3 | analogue **answerer** | ANSpcm detected for 1 s (or, **MAY**, as soon as ANSpcm is detected if the answerer sent ANSam in 9.2.3.1) | ≥ 50 ms, and until ANSpcm is no longer detected; then 75 ± 5 ms of silence, then Phase 2 |
| 9.2.2.3, 9.2.4.3 | *(receiver)* digital modem sending ANSpcm | detects TONEq | stops ANSpcm, 75 ± 5 ms of silence, then Phase 2 |
| 9.2.3.4 | *(receiver)* analogue answerer sending ANSam | detects TONEq (and also watches for CM) | stops ANSam, 75 ± 5 ms of silence, then Phase 2 |

Figures 3 and 4 mark the gap between ANSpcm and TONEq as "≤ 1 s", because TONEq may start as soon as ANSpcm
is detected. Figures 5 and 6 mark it as "1 s".

---

## 8. Requirements added by the 8.2 tables themselves

P1-24 (Tables 2 to 5, "see 9.2.5"). **P = 1 asks for LAPM per V.42.** 9.2.5 (p.50): **if both modems have
indicated LAPM capability, the V.42 ODP/ADP exchange SHALL be bypassed.**

- The analogue modem SHALL set P truthfully.
- It SHALL record the far end's P from QC1d, QCA1d, QC2d or QCA2d (or from QC1a, QCA1a, QC2a or QCA2a when
  both modems are analogue).
- If both are 1, the LAPM start-up SHALL skip ODP/ADP. LAPM then begins directly with XID/SABME (see the V.42
  digest).

P1-25 (Table 2). Bits 21 and 22 identify the signal. **The receiver SHALL use them together with the channel
and the sync to tell QC1a, QCA1a, QC1d and QCA1d apart** (section 10.1). [DERIVED as a rule; the fields are
normative]

P1-26 (Tables 3 and 5). Bit 12 is reserved and SHALL be sent as 0. [DERIVED] Receivers should ignore it, in
keeping with V.8 and V.8 bis compatibility practice.

---

## 9. Where the 8.2 signals are used: short Phase 1 for the analogue modem (9.2, pp.46 to 50)

This is a condensed restatement for context. The procedure digest is authoritative.

### 9.1 Analogue modem as caller (9.2.1)

1. At the start, listen for **ANSam** (V.8) and, **optionally**, **CRe** (V.8 bis).
2. **9.2.1.1.** Once ANSam has been detected for **1 s**:
   - Send **QC1a, then CM** at once.
   - Listen for **QCA1d, QCA1a and JM** (all on V.21(H)).
   - **On QCA1d:** stop CM **without finishing the current octet**. Send silence. Listen for **QTS, QTS\, then
     ANSpcm**. Go to 9.2.1.3.
   - **On QCA1a:** stop CM the same way. Stay silent **until ANSam is detected**, then send **TONEq**. Go to
     9.2.1.4.
   - **On JM:** continue under V.8 (full Phase 1, CM already in progress).
3. **9.2.1.2.** Once the **first 50 ms of CRe** have been detected:
   - Send **QC2a, then silence**.
   - Listen for **QCA2d, QCA2a, ANSam and ANS**.
   - **On QCA2d:** listen for QTS, QTS\, then ANSpcm. Go to 9.2.1.3.
   - **On QCA2a:** listen for ANSam. Once ANSam has been detected **for 1 s**, send TONEq. Go to 9.2.1.4.
   - **On ANSam:** go to 9.2.1.1 (QC1a + CM).
   - **If ANS is detected for 3 s after QC2a:** continue under V.8.
   - **If neither QCA2d nor QCA2a has arrived 1 s after QC2a:** continue under V.8 bis.
4. **9.2.1.3.** Once ANSpcm has been detected for **1 s**, send TONEq for **at least 50 ms**. MAY start it
   as soon as ANSpcm is detected if ANSam was already detected for 1 s in 9.2.1.1. When ANSpcm stops, stop
   TONEq, send **75 ± 5 ms** of silence, and go to Phase 2 (V.92/V.90 Phase 2).
5. **9.2.1.4.** When ANSam stops, stop TONEq, send **75 ± 5 ms** of silence, and go to **Phase 2 of V.34**
   (both modems analogue: Figures 7 and 8). V.34 11.2 then starts with INFO0 (the caller's INFO0c, then Tone
   B) after that silence.

### 9.2 Analogue modem as answerer (9.2.3)

1. After going off-hook, stay **silent for ≥ 200 ms**. Then send **ANSam** (V.8) or **CRe** (V.8 bis).
2. **9.2.3.1.** If ANSam was sent (even when an earlier V.8 bis session has timed out), listen for **QC1d,
   QC1a or CM**:
   - **On QC1d:** send **QCA1a, then silence**. Listen for QTS, QTS\, then ANSpcm. Go to 9.2.3.3.
   - **On QC1a:** **MAY** send **QCA1a**, then **75 ± 5 ms** of silence, then **ANSam**, and go to 9.2.3.4.
     Otherwise keep going with V.8. The caller sends CM right after QC1a anyway.
   - **On CM:** normal V.8.
3. **9.2.3.2.** If CRe was sent, listen for **QC2d and V.8 bis signals**:
   - **On QC2d:** stop CRe, send **QCA2a, then silence**, listen for QTS, QTS\, ANSpcm, and go to 9.2.3.3.
   - **On QC2a:** **MAY** send **QCA2a**, then 75 ± 5 ms of silence, then ANSam, and go to 9.2.3.4.
   - **On any other V.8 bis signal:** normal V.8 bis.
   - **If nothing has arrived 3 s after CRe:** send ANSam and go to 9.2.3.1.
4. **9.2.3.3.** Once ANSpcm has been detected for **1 s**, send TONEq for **≥ 50 ms**. MAY start it as soon as
   ANSpcm is detected if ANSam was sent in 9.2.3.1. When ANSpcm stops, stop TONEq, send **75 ± 5 ms** of
   silence, and go to Phase 2.
   - **If no ANSpcm arrives within 2 s after QCA1a:** send ANSam and continue under **V.8**.
   - **If no ANSpcm arrives within 2 s after QCA2a:** send ANSam and go to **9.2.3.1**.
5. **9.2.3.4.** While sending ANSam, listen for **TONEq and CM**:
   - **On CM:** V.8.
   - **On TONEq:** stop ANSam, send **75 ± 5 ms** of silence, and go to Phase 2 (V.34 Phase 2, since both
     modems are analogue).

### 9.3 Timer and tolerance table (analogue modem, short Phase 1)

| Value | Where | Meaning |
|---|---|---|
| 300 bit/s | 8.2 | QC/QCA bit rate |
| 980 / 1180 Hz; 1650 / 1850 Hz (±6 Hz tx, ±12 Hz rx) | V.21 | V.21(L) and V.21(H) mark / space |
| ±0.01% | 7.2/V.8 bis | V.8 bis message F_A and F_Z |
| 980 Hz | 8.2.5 | TONEq |
| 200 ms | Table 2 | QC1a duration (60 bits) |
| 233.3 ms | Table 4 | QCA1a duration (70 bits) |
| 100 ms ± 2% | 7.2.4/V.8 bis | QC2a / QCA2a mark preamble |
| 2 to 5 / 1 to 3 flags | 7.2.5/V.8 bis | opening / closing flags |
| 1 s | 9.2.1.1 | ANSam must be detected this long before QC1a |
| 50 ms | 9.2.1.2 | initial part of CRe that must be detected before QC2a (Figure 5: "≥ 50 ms") |
| 1 s | 9.2.1.2 | no QCA2d or QCA2a within this time after QC2a → V.8 bis |
| 3 s | 9.2.1.2 | ANS detected this long after QC2a → V.8 |
| 1 s | 9.2.1.2 | ANSam must be detected this long after QCA2a before TONEq |
| 1 s | 9.2.1.3, 9.2.3.3 | ANSpcm must be detected this long before TONEq (unless the "MAY" shortcut applies) |
| ≥ 50 ms | 9.2.1.3, 9.2.3.3 | minimum TONEq duration |
| 75 ± 5 ms | 9.2.1.3, 9.2.1.4, 9.2.3.1 to 9.2.3.4 | silence at the end of Phase 1; silence between QCA1a/QCA2a and ANSam |
| ≥ 200 ms | 9.2.3 | answerer's initial silence |
| 3 s | 9.2.3.2 | CRe with no response → ANSam |
| 2 s | 9.2.3.3 | no ANSpcm after QCA1a / QCA2a → ANSam |
| 768T = 96 ms, 48T = 6 ms | 8.3.6, Figures 3 to 6 | QTS and QTS\ durations (received) |
| 37.625 ms period; reversal every 451.5 ms | 8.3.1 | ANSpcm (received) |
| 5 ± 1 s | 8.2.2/V.8 | ANSam length when not cut short |

---

## 10. Signals the analogue modem must decode (from 8.3, for reference)

### 10.1 QC1d and QCA1d (Tables 11 and 13, rendered pp.21 and 22)

Same V.8-type skeleton as section 5.1. QC1d is on **V.21(L)**, sent once, and is followed immediately by CM.
QCA1d is on **V.21(H)**, sent once, and ends with ten ONEs (70 bits).

| Bit | QC1d | QCA1d |
|---|---|---|
| 20 | `0` start | `0` start |
| 21 | **`1` digital modem** | **`1` digital modem** |
| 22 | `0` QC | `1` QCA |
| 23 | `P` | `P` |
| 24:29 | `000LM1` | `000LM1` |
| 50:59 | `010P000LM1` | `011P000LM1` |
| 60:69 | — | `1111111111` |

**LM = the level of the ANSpcm the digital modem will send.** L is bit 27 and M is bit 28 (L first):

| LM | ANSpcm level |
|---|---|
| `00` | −9.5 dBm0 |
| `01` | −12 dBm0 |
| `10` | −15 dBm0 |
| `11` | −18 dBm0 |

Octet: `0x01 | A<<1 | P<<2 | L<<6 | M<<7`. Examples: QC1d with P=1, LM=00 is 0x05; QCA1d with P=0, LM=11 is 0xC3.

QC1d and QCA1d **carry no U_QTS**. The digital modem does not choose the QTS level.

**Discrimination table for a V.21 receiver in the analogue modem:**

| Channel heard | Sync after ≥ 10 ONEs | Bit 21 | Bit 22 | Signal |
|---|---|---|---|---|
| V.21(L) (answerer hears it) | `0101010101` | 0 | 0 | QC1a (both modems analogue) |
| V.21(L) | `0101010101` | 1 | 0 | QC1d |
| V.21(L) | `0000001111` | — | — | CM |
| V.21(H) (caller hears it) | `0101010101` | 0 | 1 | QCA1a |
| V.21(H) | `0101010101` | 1 | 1 | QCA1d |
| V.21(H) | `0000001111` | — | — | JM |

A bit-22 value that does not match the listening state (for example, a QC heard on V.21(H)) is not defined
by V.92. Treat it as invalid.

### 10.2 QC2d and QCA2d (Tables 12 and 14, rendered pp.21 and 22)

These use the V.8 bis framing of section 6. QC2d is on **V.21(H)** and QCA2d on **V.21(L)**. Identification
field:

| Bit | QC2d | QCA2d |
|---|---|---|
| 0:3 | `1011` | `1011` |
| 4:7 | `VVVV` (ignore) | `VVVV` (ignore) |
| 8:9 | `LM` (ANSpcm level, as in Table 11) | `LM` |
| 10:12 | `000` reserved | `000` reserved |
| 13 | `P` | `P` |
| 14 | `0` QC | `1` QCA |
| 15 | **`1` digital modem** | **`1` digital modem** |

Octet 2 = `L | M<<1 | P<<5 | Q<<6 | 0x80`.

- The V.8 bis messages QC2a, QCA2a, QC2d and QCA2d are told apart by **octet 2, bits 7 and 8** (V.92 bits 14
  and 15).
- They are told apart from other V.8 bis messages by the **message-type nibble 0xD**.
- Note that 8.3.5 says "the analogue modem shall encode" Table 14 (QCA2d). That is a typo for the digital
  modem.

### 10.3 QTS and QTS\ (8.3.6, p.23)

- **QTS** = **128 repetitions** of `{+V, +0, +V, −V, −0, −V}` = **768 symbols** (96 ms).
- **QTS\** = **8 repetitions** of `{−V, −0, −V, +V, +0, +V}` = **48 symbols** (6 ms).
- **V** = the PCM codeword with Ucode **U_QTS** (the value the analogue modem sent in QC1a, QCA1a, QC2a or
  QCA2a). **0** = the codeword with Ucode 0.
- The first QTS symbol is in **data frame interval 0**. The digital modem keeps data frame alignment from that
  point.
- Sequence from the digital modem (Figures 3 to 6): QCA1d or QCA2d (or CM in Figure 4; nothing in Figure 6),
  then **75 ± 5 ms** of silence, then QTS, then QTS\, then ANSpcm.

[DERIVED] Facts useful to the analogue receiver:

- The 6-symbol period puts the fundamental at 8000/6 = **1333.3 Hz**. QTS\ is QTS with its sign flipped, so the
  QTS → QTS\ boundary is a 180° phase reversal. That reversal is the obvious timing mark for finding frame
  interval 0: QTS\ starts at symbol 768, and ANSpcm at symbol 816.
- In **µ-law**, Ucode 0 is `FF` (+0) and `7F` (−0), with linear value 0.
- In **A-law**, Ucode 0 is `D5`, linear **+8**, and its negative is **−8**. So "0" is not exactly zero on an
  A-law link.

### 10.4 ANSpcm (8.3.1, Table 6, p.16)

- A repeating PCM codeword sequence that gives a tone of about **2100 Hz**. [DERIVED] The exact value is
  8000·79/301 = **2099.67 Hz**.
- Period **301 symbols** (37.625 ms). A **phase reversal** is added every **3612 symbols** (= 12 periods =
  **451.5 ms**).
- It MAY be used to check that the assumed channel characteristics are right.
- It SHALL be sent at one of four levels. The level is signalled by LM.

| Level | scl µ-law | scl A-law | ϑ |
|---|---|---|---|
| −9.5 dBm0 | 1334 | 667 | 0.25·π/301 |
| −12 dBm0 | 1000 | 500 | 0.25·π/301 |
| −15 dBm0 | 708 | 354 | 0.25·π/301 |
| −18 dBm0 | 500 | 250 | 0.25·π/301 |

- Generation: x = ⌊scl·√2·cos(2πk·79/301 + ϑ) + 0.5⌋ for k = 0 to 300, quantised to a G.711 linear value. The
  result SHALL equal Tables 7 to 10 (PDF pp.17 to 20). That is the digital modem's job.
- NOTE: some network equipment changes the channel characteristics when it hears ANSpcm.
- [DERIVED] For the analogue detector: ANSpcm is a 2100 Hz tone with ~450 ms phase reversals and **no 15 Hz
  AM**. This distinguishes it from ANSam. It is spectrally almost the same as V.25 **ANS with phase
  reversals**. See pitfall 12.4.

---

## 11. Receiver design notes for the 8.2 and 8.3 FSK signals [DERIVED; V.92 gives no detection criteria]

1. **Channel.** Each state listens on exactly one V.21 channel:
   - analogue caller after QC1a or QC2a: **V.21(H)**;
   - analogue answerer: **V.21(L)**.
   The existing V.8 CM/JM demodulators can be reused as they are.
2. **V.8-type frames:**
   - Hunt for **≥ 10 consecutive ONEs** followed directly by the 10-bit sync.
   - If the sync is `0101010101`, take the next 10 bits as the info frame. Check start = 0, bit 25 = 0 and
     stop = 1.
   - Then expect ONEs, the sync again, and the repeated frame (bits 50:59).
   - Since the signal is sent **once**, the two copies are the only redundancy. **Recommendation:** accept only
     when both copies agree. This mirrors V.8's "two identical sequences" rule for CM and JM, but V.92 does
     not require it. See Q3.
3. **V.8 bis frames:**
   - Flag hunt, then unstuffing, then FCS check (residue 0xF0B8 in reflected form).
   - Check at least 3 octets between flags (2 information + 2 FCS = 4 for QC2).
   - Check nibble 0xD, then decode octet 2.
4. **Stopping CM "without completing the current octet"** (9.2.1.1). The caller cuts its V.21(L) transmitter
   off mid-octet and goes silent. **No CJ is sent in short Phase 1.** The digital modem does the same in
   9.2.2.1.
5. **QC1a → CM continuity.** Keep the FSK phase continuous across the QC1a/CM boundary, as within one V.21
   transmission. V.92 does not say so, but a phase jump would disturb the far end's V.21 demodulator in the
   middle of the stream.
6. **TONEq detector (the answerer in 9.2.3.4):**
   - It must not fire on V.21(L) marking inside CM or QC1a/QC1d. Those contain runs of 980 Hz. The longest is
     up to three 1 option bits (b5 to b7) of the last octet, plus its stop bit, plus the 10 preamble ONEs of
     the next repetition: **14 bits = 46.7 ms**. QC1a with WXYZ = `1111` really has this run (bits 26 to 39).
   - A caller may still be sending CM when the answerer turns ANSam back on, because the caller stops CM only
     after it detects QCA1a, and there is transmission delay.
   - **Recommendation:** require **≥ 60 ms** of steady 980 Hz with no 1180 Hz energy, and keep the V.8 CM
     detector running in parallel.
   - The same caution applies to the digital modem's TONEq detector in 9.2.2.3 and 9.2.4.3.
7. **ANSam "detected for 1 s"** needs the 15 Hz AM check, which `crates/v8/src/ansam.rs` already does. The
   separate ANS-vs-ANSpcm question is in pitfall 12.4.

---

## 12. Implementation notes

### 12.1 What is new compared with V.90

- V.90 has only full Phase 1: after ANSam, Te (≥ 0.5 s), then at least two CMs and two JMs, then CJ. V.92
  adds **short Phase 1**. The caller signals quick-connect **inside the first 200 ms of V.21 traffic** (QC1a), and still sends
  CM afterwards as a fallback. So a non-V.92 answerer sees ordinary V.8, plus 200 ms of V.21(L) with an
  unknown sync, which it ignores.
- The V.8 sync `0101010101` (octet 0x55) is **reserved for V.92** in V.8 (2000) Table 1.
- V.8 bis message type `1101` (bits 4 to 1) is **reserved for V.92** in V.8 bis (2000) Table 3.
- The analogue modem chooses **U_QTS**, the PCM level of the digital modem's QTS training tone. Nothing like
  this exists in V.90.
- The analogue modem must **detect PCM-generated signals before Phase 2**: QTS, QTS\ and ANSpcm.
- **TONEq** (980 Hz) is a new acknowledgement tone.
- **LAPM negotiation moves into Phase 1**. The P bit plus 9.2.5 means ODP/ADP is skipped whenever both modems
  set P, even without a V.8 prot0 octet.
- **Cleardown of a held call** uses a QC with U_QTS = `1111`.
- Both-analogue short Phase 1 (Figures 7 and 8) goes straight to **V.34 Phase 2** (INFO0c/INFO0a), skipping
  CM/JM/CJ.

### 12.2 Reuse inside BinModem

- `crates/v8`: the V.8 async framer, the 0x55 sync constant (new), the ANSam detector, and the V.21 channel
  choice.
- `crates/ec/src/hdlc.rs`: HDLC FCS and bit stuffing, for the V.8 bis QC2 messages. There is no V.8 bis
  implementation yet; only the QC2 subset is needed (plus CRe detection).
- `crates/datapump/src/v8.rs` and `v34/startup.rs`: V.21 modulators and demodulators, and the V.34 Phase 2
  entry.

### 12.3 Encoding checklist (analogue modem)

```
QC1a  : preamble(10 ones) 0x55 info 1111111111 0x55 info            info = P<<2|W<<3|X<<5|Y<<6|Z<<7
QCA1a : preamble(10 ones) 0x55 info 1111111111 0x55 info 1111111111 info = 0x02|P<<2|W<<3|X<<5|Y<<6|Z<<7
QC2a  : 100 ms marks, 2..5 flags, [0x2D, W|X<<1|Y<<2|Z<<3|P<<5], FCS(lo,hi), 1..3 flags   (V.21(H))
QCA2a : 100 ms marks, 2..5 flags, [0x2D, W|X<<1|Y<<2|Z<<3|P<<5|0x40], FCS, 1..3 flags     (V.21(L))
TONEq : 980 Hz continuous
```

All octets go on the line LSB first (async framing adds start 0 and stop 1 for QC1a and QCA1a only). The
info octet is `bit21..bit28`.

### 12.4 Pitfalls

1. **Bit order of WXYZ, VVVV and `1011`.** They are patterns sent **leftmost first** (clause 8 rule). The V.8
   bis message-type nibble is **0xD** and the revision nibble is **2**. Writing 0xB and 4 into the octet is
   the classic mistake.
2. **The W position in QC1a is bit 24, then a fixed 0 at bit 25, then X, Y, Z at bits 26 to 28.** WXYZ is not
   contiguous in QC1a and QCA1a. It is contiguous (bits 8 to 11) in QC2a and QCA2a.
3. **The extracted text is wrong for Tables 2, 4, 11, 13 and 6.** It places `W0XYZ1` at bits 30:39 in Table 4,
   merges the LM sub-table into the bit column in Tables 11 and 13, and scrambles the ANSpcm µ-law and A-law
   scl columns in Table 6. Use this digest or the
   rendered pages.
4. **ANS vs ANSpcm confusion** (9.2.1.2). After QC2a, the caller listens for "ANS for 3 s → V.8".
   - If the caller misses QCA2d, the digital modem's ANSpcm (plain 2100 Hz with ~450 ms reversals, no AM)
     looks like ANS.
   - If the caller then drops to V.8 after 3 s, both ends lose the call: the digital modem gives up after 2 s
     without TONEq and sends ANSam.
   - **Recommendation:** in 9.2.1.2, treat a no-AM 2100 Hz tone that starts right after about 75 ms of
     silence and a 1333 Hz QTS burst as ANSpcm. Otherwise run the 3 s ANS timer only when no QTS was seen.
5. **Sync look-alikes** (section 5.1). With P = 0, the QC1a info frame can equal the CI sync (WXYZ = `0000`)
   or the CM/JM sync (WXYZ = `0111`). Anchor syncs to the ONEs preamble.
6. **TONEq = V.21(L) mark** (sections 7 and 11 item 6).
7. **QC2a overlaps CRe** (Figure 5). The answerer must decode V.21(H) while sending CRe segment 1 (1375 + 2002
   Hz) or segment 2 (400 Hz). The 2002 Hz component sits about 150 Hz above the V.21(H) space tone (1850 Hz). The echo of CRe
   in the answerer's own receiver needs filtering.
8. **U_QTS = `1111` must never appear in a normal QC.** It clears down an on-hold modem.
9. **No CJ in short Phase 1.** Only the full V.8 path ends CM with CJ.
10. **The analogue answerer's QCA1a or QCA2a to an analogue caller is optional** ("may"). A V.92 analogue
    answerer that does not implement V.34 short start simply continues with V.8. The caller is still sending
    CM, so V.8 completes normally.
11. **Timing reference for "detected for 1 s".** Measure from the start of reliable detection, not from the
    start of the tone. Allow for VoIP jitter slips on the test rig (project memory: about 20 ms concealment
    inserts every few seconds; about 1.5 s round trip on the VoIP line). A slip inside the 1 s ANSam window
    must not reset the timer.
12. **Echo from the analogue caller's own QC1a and CM** is on V.21(L), and the caller listens on V.21(H), so
    there is no self-detection risk. The analogue answerer sends QCA1a on V.21(H) and listens on V.21(L), so
    the same holds.

### 12.5 Ambiguities and open questions

- **Q1 (levels).** 8.2 gives no transmit level for QC*, QCA* or TONEq. Proposal: nominal transmit power, as for
  every other Phase 1 signal (V.90 8.1, V.34 10.1.1).
- **Q2 (TONEq tolerance and detection time).** None are given. Proposal: generate exactly 980 Hz. Detect with
  a ≥ 60 ms steady-state rule that rejects V.21(L) data.
- **Q3 (QC/QCA acceptance).** Is one valid copy of the info frame enough, or must both copies (bits 20:29 and
  50:59) agree? V.92 is silent. Proposal: require both, with a fallback to one copy plus good preamble and sync
  if the other copy has a framing error. Rationale: a false QC is costly, but a missed QC only costs the V.8
  fallback.
- **Q4 (QC2 information field length).** Are S and NS fields allowed or expected after the 2-octet
  identification field? The V.8 bis delimiting bit (octet 2, bit 8) conflicts with the V.92 analogue/digital
  flag. Proposal: send 2 octets; accept and ignore extra octets.
- **Q5 (V.8 bis error handling).** If a QC2 or QCA2 frame fails its FCS, should the receiver send NAK(1) as
  in 9.8/V.8 bis? 9.2 only defines timeouts (1 s after QC2a → V.8 bis). Proposal: do not NAK. Let the 1 s,
  2 s and 3 s timers handle it.
- **Q6 (U_QTS policy).** How should the analogue modem choose among Ucodes 61 to 87? V.92 gives no guidance.
  Proposal: a fixed mid value (for example Ucode 70 or 71) until Rory's captures show what real digital modems
  and transcoding paths do with QTS. What should QCA1a or QCA2a carry when the far end is analogue (no QTS)?
- **Q7 (TONEq trigger after QCA1a).** 9.2.1.1 says send TONEq once ANSam "is detected", with no 1 s
  qualifier. 9.2.1.2 (after QCA2a) requires 1 s. Figure 7 shows no duration. Proposal: use a short
  confirmation (about 100 to 200 ms of ANSam with AM) after QCA1a, and 1 s after QCA2a, as written.
- **Q8 (typos in the related 9.2.2.2 and 8.3.5).**
  - 9.2.2.2 (digital caller) says "1 s after transmitting QC2a"; it must mean QC2d.
  - 8.3.5 says "the analogue modem shall encode" QCA2d; it must mean the digital modem.
  Neither changes the analogue modem.
- **Q9 (CRe detection by the analogue caller).** V.92 says detect the "initial 50 ms" of CRe. The segment 1
  dual tone may itself be only 285 ms long, and V.8 bis also has MRe with the same segment 1. MRe and CRe
  differ only in segment 2 (650 Hz vs 400 Hz), which starts 285 to 400 ms later. So a 50 ms trigger cannot
  tell CRe from MRe. Proposal: send QC2a on any initiating dual tone of 50 ms. If segment 2 turns out to be
  MRe, fall back to V.8 bis handling when QCA2x does not arrive within 1 s (9.2.1.2).
- **Q10 (full Phase 1 CM content for V.92).** V.90 9.1.1 (1998) speaks of a "V.90 availability category";
  V.8 (2000) renamed it PCM modem availability (`pcm0`, b5 = analogue V.90/V.92). The V.92 NOTE confirms
  there is no V.92-specific V.8 bit. The analogue modem's CM is therefore the same as V.90's: callf0 data
  (`0xC1`), modn0 with b5 and b6 set (`0x65`, plus b7 if half-duplex), pcm0 with b5 (`0x27`), access0 (`0x0D`,
  or `0x8D` on a digital connection), and optionally prot0 LAPM (`0x2A`). These octet values are [DERIVED]
  from V.8 Tables 2 to 7. Check them against `crates/v8`.
