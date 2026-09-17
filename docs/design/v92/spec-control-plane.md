# V.92 control plane: V.8, V.8 bis, V.250 and V.42/V.44 (implementation digest)

This digest covers what V.92 needs from Recommendations **other than** V.92 itself:

- **V.8 (11/2000)**: menus, octets and the V.92 synchronisation pattern.
- **V.8 bis (11/2000)**: signals, message framing and the V.92 capability bits.
- **V.250 (07/2003)**: clause 6.8 `+P` commands, plus the `+MS`/`+MR`/`+ES`/`+DR`/`+TMO` material that concerns V.92.
- **V.42 (03/2002)** and **V.44 (11/2000)**: how they tie to V.92.

V.92's short Phase 1 signals are built from V.8 and V.8 bis formats, so section 3 restates them in those formats. It adds octet values and FCS vectors. The full V.92 treatment of the other phases is in the sibling `spec-*.md` files in this folder.

---

## 0. Sources and conventions

### 0.1 Pages read

All pages were rendered at 170 dpi with PyMuPDF and viewed. Page numbers are **1-based PDF pages**, not printed page numbers.

| Document | PDF pages read (rendered) | Printed page = PDF page minus |
|---|---|---|
| T-REC-V.8-200011 | 9, 10, 11, 12, 13, 14, 15, 16, 17 | 5 |
| T-REC-V.8bis-200011 | 13, 14, 15, 16, 19, 20, 21, 22, 24, 25, 35, 36, 38, 39, 40 | 7 |
| T-REC-V.250-200307 | 56, 78, 91, 92, 93, 94, 95, 96, 97, 98, 100 | 6 |
| T-REC-V.92-200011 | 14, 15, 16, 21, 22, 45, 46, 47, 48, 49, 50, 51 | 7 |
| T-REC-V.42-200203 | 59, 67 | 8 |
| T-REC-V.44-200011 | 32, 33 | 7 |

Some prose was read from the PyMuPDF text layer, and every numeric value in it was checked against a rendered page:

- V.250 pp.49-50 (H, O), 54-59 (+MS, +MA, +MR), 66 (+ES), 11 (circuit 125), 46 (the `!` dial modifier), 48 (RING), 29 (+GCAP).
- V.42 pp.15, 23 (L-SUSPEND/L-RESUME).
- V.90 1998, clause 9.1.1.

### 0.2 Markers and bit order

- **[DERIVED]** marks a value computed here, such as an octet value, a byte image or an FCS, or an inference. None of these is printed in a Recommendation.
- **V.8 octets.** Bits are written b0..b7. Clause 5.1 lists them in transmission order after the start bit, and says b0 is the least significant bit of the tag. The hex values here use **value = sum of b_i·2^i**, which is the convention `crates/v8` already uses (for example prot0 = LAPM = `0x2A`).
- **V.8 bis octets.** Bits are numbered 1..8, and bit 1 is sent first (7.2.2). Within one octet, bit 1 is the LSB. Hex values here use **value = sum of bit_n·2^(n-1)**.
- **V.92 bit patterns.** Short Phase 1 tables (Tables 2-5, 11-14) give bit patterns with the **left-most bit first in time** (V.92 clause 8, first paragraph).

---

## 1. Answers in brief

1. **V.8.** There is **no V.92-specific CM/JM bit**. V.92 uses the same octets as V.90:
   - `modn0` b5 = 1;
   - `pcm0` b5 or b6 ("V.90 **or V.92** analogue/digital");
   - `access0` present;
   - the V.34 duplex bit set.

   V.92 9.1 NOTE says plainly that V.8 has no means of indicating V.92 alone. V.92 capability is settled in Phase 2 by INFO0d bit 27 and INFO0a bit 26.

   What V.8 (2000) adds for V.92:
   - Table 1 reserves the synchronisation pattern **`0101010101`** as "defined in V.92". It is the sync of QC1a, QCA1a, QC1d and QCA1d.
   - 7.3 and 7.4 add the note that when both ends set **prot0 = LAPM**, a following Recommendation may *require* ODP/ADP to be skipped. The example given is 9.3.1/V.92.
2. **V.8 bis.** Revision 2 adds, in the **Data NPar(2) octet 4** (Table 6-3d):
   - bit 2 = V.92 analogue modem;
   - bit 3 = V.92 digital modem.

   It also reserves message type **`1101`** (bits 4..1) for V.92. V.92 QC2a, QCA2a, QC2d and QCA2d use that type.

   **Short Phase 1 does not depend on V.8 bis.** The QC1x/QCA1x pair runs entirely inside a V.8 (ANSam) start. V.8 bis is needed only if the answering modem chooses to send **CRe**. In that case the caller responds with QC2x, and the answerer replies with QCA2x. A caller that cannot detect CRe loses about 3 s, because the answerer then falls back to ANSam (V.92 9.2.3.2, 9.2.4.2).
3. **V.250 6.8** defines eight commands:

   | Command | Kind | Default |
   |---|---|---|
   | `+PCW` | parameter | 0 |
   | `+PMH` | parameter | 0 |
   | `+PMHT` | parameter | **none stated** |
   | `+PMHR` | action; reply `+PMHR: <n>`, possibly delayed | none |
   | `+PIG` | parameter | 0 |
   | `+PMHF` | action-like; no values | none |
   | `+PQC` | parameter | 0 |
   | `+PSS` | parameter | 0 |

   All eight are **mandatory if the DCE implements V.92**. `+MS` gains the carrier string **`V92`** (Table 13). `+TMO` (6.9) retrieves V.59 objects and gives `+TMO V92 All` and `+TMO V92 rxHistory` as examples. V.59 is not in `docs/specs`.
4. **V.42 and V.44.**
   - V.92 needs **LAPM without the detection phase** when both ends said LAPM (9.2.5 and 9.3.1).
   - V.42 (2002) adds **L-SUSPEND/L-RESUME** (7.10/7.11), which freeze and unfreeze timers across retrains and V.92 modem-on-hold.
   - V.92 has **no normative link to V.44**. It only says data compression "may also be employed" (7.2).
   - V.44 is negotiated inside the V.42 XID user data subfield (Table 11c/V.42, Annex A/V.44), as for any V.42 link.

---

## 2. ITU-T V.8 (11/2000)

### 2.1 Coding format (clause 5)

- CI, CM and JM share one format. Each sequence is:
  1. **ten ONEs**;
  2. **ten synchronisation bits**;
  3. information octets, each framed as start bit 0, eight bits, stop bit 1.
- The coding keeps the HDLC flag `01111110` out of the bit stream, so JM (on V.21(H)) cannot be mistaken for T.30 HDLC.
- The **first category in a sequence is always the call function**. The other categories may come in any order. A category is one octet, or an ordered run of octets.

**Category octet (5.1)**, in transmission order:

```
start(0) b0 b1 b2 b3 | b4=0 | b5 b6 b7 | stop(1)
         \-- tag --/  category  option bits
         (b0 = LSB)
```

**Extension octet (5.2)**. Any number of these may follow a category octet directly:

```
start(0) b0 b1 b2 | b3=0 b4=1 b5=0 | b6 b7 | stop(1)
         options     marker        options   -> 5 option bits: b0,b1,b2,b6,b7
```

- b4 = 1 marks an extension octet. b3 = b5 = 0 prevents flag simulation.
- [DERIVED] An octet is an extension octet when `(o & 0x38) == 0x10`, and a category octet when `(o & 0x10) == 0`.
- **Clause 6 / clause 10.** Categories, extension octets and bits not defined in clause 6 are reserved. A receiver **shall ignore** them. Proprietary information goes only in the NS field (6.6).

### 2.2 Table 1/V.8: preamble and synchronisation (rendered p.9)

| Bits (transmission order) | Meaning | [DERIVED] as a framed octet |
|---|---|---|
| `1111111111` | ten ONEs before every sequence | idle marks |
| `0000000001` | sync for **CI** | start 0, data `0x00`, stop 1 |
| `0000001111` | sync for **CM and JM** | start 0, data `0xE0`, stop 1 |
| `0101010101` | "**Defined in ITU-T V.92**" | start 0, data **`0x55`**, stop 1 |

- The third pattern is the sync of QC1a, QCA1a, QC1d and QCA1d (V.92 Tables 2, 4, 11, 13). See section 3.
- [DERIVED] `0x55` is also a valid V.8 **extension** octet, because b3 = 0, b4 = 1 and b5 = 0. A receiver can tell it is a sync only by where it sits: it is the first octet after a run of at least ten ONEs.

### 2.3 Table 2/V.8: category tags (rendered p.10)

| Category | b0 b1 b2 b3 | Tag value | [DERIVED] octet with no options |
|---|---|---|---|
| Call function | 1 0 0 0 | 1 | `0x01` |
| Modulation modes | 1 0 1 0 | 5 | `0x05` |
| Protocols | 0 1 0 1 | 10 | `0x0A` |
| PSTN access | 1 0 1 1 | 13 | `0x0D` |
| Non-standard facilities | 1 1 1 1 | 15 | `0x0F` |
| PCM modem availability | 1 1 1 0 | 7 | `0x07` |
| Defined in T.66 | 0 1 1 1 | 14 | `0x0E` |

In every row, b4 = 0 and b5..b7 are the options.

### 2.4 Table 3/V.8: call function `callf0` (rendered p.10)

Tag `1000`, b4 = 0. The options b5 b6 b7 are:

| b5 b6 b7 | Call function | Reference | [DERIVED] octet |
|---|---|---|---|
| 0 0 0 | to be determined by ITU-T | | `0x01` |
| 1 0 0 | PSTN multimedia terminal | H.324 | `0x21` |
| 0 1 0 | Textphone | V.18 | `0x41` |
| 1 1 0 | Videotext | T.101 | `0x61` |
| 0 0 1 | Transmit facsimile from call terminal | T.30 | `0x81` |
| 1 0 1 | Receive facsimile at call terminal | T.30 | `0xA1` |
| **0 1 1** | **Data (unspecified application)** | V-series modems | **`0xC1`** |
| 1 1 1 | call function given in an extension octet | | `0xE1` |

V.92 data calls use `0xC1`.

### 2.5 Table 4/V.8: modulation modes `modn0`, `modn1`, `modn2` (rendered p.11)

The "item" number is what 7.4 uses to choose among non-PCM modes: **the lowest item wins**.

| Octet | Bit | Meaning | Item | [DERIVED] mask |
|---|---|---|---|---|
| `modn0` (category, tag `1010`, b4 = 0) | b5 | **1 when the PCM modem availability category is present** | 0 | `0x20` |
| | b6 | V.34 duplex | 1 | `0x40` |
| | b7 | V.34 half-duplex | 2 | `0x80` |
| `modn1` (extension: b3 b4 b5 = 0 1 0) | b0 | V.32 bis / V.32 | 3 | `0x01` |
| | b1 | V.22 bis / V.22 | 4 | `0x02` |
| | b2 | V.17 | 5 | `0x04` |
| | b6 | V.29 half-duplex (as in T.30) | 6 | `0x40` |
| | b7 | V.27 ter | 7 | `0x80` |
| `modn2` (extension: b3 b4 b5 = 0 1 0) | b0 | V.26 ter | 8 | `0x01` |
| | b1 | V.26 bis | 9 | `0x02` |
| | b2 | V.23 duplex | 10 | `0x04` |
| | b6 | V.23 half-duplex | 11 | `0x40` |
| | b7 | V.21 | 12 | `0x80` |

- Base values [DERIVED]: `modn0` = `0x05 | options`; `modn1` = `modn2` = `0x10 | options`.
- 6.2: a mode is shown only if it can be used with the indicated call function and the DCE wants to advertise it.

### 2.6 Table 5/V.8: PCM modem availability `pcm0` (rendered p.12)

Tag `1110` (value 7), b4 = 0.

| Bit | Meaning | [DERIVED] mask |
|---|---|---|
| b5 | **V.90 or V.92 analogue modem** available | `0x20` |
| b6 | **V.90 or V.92 digital modem** available | `0x40` |
| b7 | V.91 available | `0x80` |

[DERIVED] Resulting octets: analogue only = `0x27`, digital only = `0x47`, both = `0x67`.

Consistency rules (6.3, 7.3 and 7.4). All are "shall":

- If pcm0 **b5 or b6** is set, the modulation category **must be present** with at least the **V.34 availability bit** set (7.3 wording; 6.3 and 7.4 say "the V.34 availability bit").
- If pcm0 is present, **access0 must also be present**.
- If pcm0 is present and a modulation category is present, **modn0 b5 = 1**.
- In **JM**, pcm0 may appear **only if** CM carried it, PCM can serve the call function, and the answerer wants to advertise it (7.4).
- **If pcm0 is in JM**, operation continues per **V.90, V.91 or V.92**. Otherwise the lowest-item common mode is used (7.4).
- 8.2.3: when the answerer does not offer the requested call function, JM may name a different one. JM then keeps the same number of modulation octets as CM, all zero, and **must not carry pcm0**.

### 2.7 Table 6/V.8: protocols `prot0`, and the ODP/ADP text (rendered pp.12, 14, 15)

Tag `0101` (value 10), b4 = 0.

| b5 b6 b7 | Meaning | [DERIVED] octet |
|---|---|---|
| 1 0 0 | calls for **LAPM per V.42** | **`0x2A`** |
| 1 1 1 | protocol given in an extension octet | `0xEA` |

- NOTE to Table 6: a missing prot0 does not rule out other ways of negotiating a protocol.
- 6.4: if CM asks for LAPM and the answerer wants LAPM, JM also carries a prot0 indicating LAPM.
- 7.3 (CM) and 7.4 (JM), new in 2000:
  - prot0 lets LAPM be negotiated **without the ODP/ADP exchange** (see 7.2.1/V.42).
  - When **both** DCEs say LAPM in prot0, another Recommendation may **require** ODP/ADP to be omitted. The example is **9.3.1/V.92**.
  - V.8 also warns that some existing implementations set prot0 to LAPM but still need ODP/ADP to reach LAPM.
- 7.4: JM **may** carry prot0 only if CM indicated LAPM.

### 2.8 Table 7/V.8: PSTN access `access0` (rendered p.13)

Tag `1011` (value 13), b4 = 0.

| Bit | Meaning | [DERIVED] mask |
|---|---|---|
| b5 | the **call** DCE is on a cellular connection | `0x20` |
| b6 | the **answer** DCE is on a cellular connection | `0x40` |
| b7 | 1 = this DCE is on a **digital** network connection; 0 = on an **analogue** one | `0x80` |

- NOTE 1: no access0 means nothing is known about access.
- NOTE 2: an analogue V.90/V.92 modem may sit on a digital connection.
- 7.3 (CM): the caller includes access0 when it wants to indicate access type.
  - On a cellular connection it sets b5 = 1 and b6 = 0.
  - b7 = 1 means digital (for example an ISDN B channel carrying encoded analogue); b7 = 0 means analogue.
- 7.4 (JM): the answerer includes access0 when it wants to indicate access type, or when CM's access0 had b5 = 1.
  - b6 = 1 if the answerer is cellular.
  - **b5 = 1 in JM if and only if b5 = 1 in CM.**
  - b7 has the same meaning as in CM.
- V.92 INFO0 bit 24 (CME) "may be used in conjunction with" access0. See `spec-phase2-signals.md`.

### 2.9 Table 8/V.8: non-standard facilities (rendered p.13)

- The NS category octet is `11110xxx` (tag 15). It follows the standard fields.
- Block layout:

  | Field | Length (octets) |
  |---|---|
  | length = K+L+M+1 | 1 |
  | T.35 country code | K (no longer limited to 1) |
  | provider code length | 1 |
  | provider code | L |
  | non-standard information | M |

- The NS data are carried in extension octets, **five NS bits per extension octet**, with higher-order NS bits in higher-order b positions. Several blocks may be concatenated.
- Not needed for V.92.

### 2.10 Signal rules and timings that V.92 relies on (7.1-8.2; rendered pp.14-17)

| Item | Requirement | Clause |
|---|---|---|
| CI cadence | ON ≥ 3 CI sequences and ≤ 2 s; OFF 0.4 s to 2 s | 7.1 |
| ANSam | 2100 ± 1 Hz, AM at 15 ± 0.1 Hz, envelope 0.8 ± 0.01 to 1.2 ± 0.01 of average; phase reversals every 450 ± 25 ms (omitted when echo-canceller disabling is not wanted) | 7.2 |
| ANSam out-of-band power | ≥ 24 dB below the 2100 ± 200 Hz band | 7.2 |
| CM only after ANSam | the call DCE shall not send CM unless ANSam was detected | 7.2 |
| Caller pre-silence | 1 s of no signal, then CI/CT/CNG or nothing | 8.1.1 |
| Te | silence before CM, ≥ 0.5 s, or ≥ 1 s if V.25 echo-canceller disabling is wanted. It starts at the end of the call signal, or at ANSam detection if there was no call signal. | 8.1.1 |
| JM trigger | ≥ 2 identical CM sequences received | 7.4, 8.2.2 |
| CJ trigger | ≥ 2 identical JM sequences received; the caller finishes the current CM octet, then sends CJ (three all-zero octets with start and stop bits) | 3.5, 8.1.2 |
| After CJ | silence **75 ± 5 ms**, then sigC | 8.1.2 |
| Answerer pre-silence | ≥ 0.2 s after connecting to line | 8.2 |
| ANSam length | 5 ± 1 s unless ended by CM or sigC | 8.2.2 |
| JM end | JM continues until all three CJ octets arrive, or another criterion (sigC, CM absent), then stops without finishing the sequence; 75 ± 5 ms silence; sigA | 8.2.3 |
| No common mode | if JM has no pcm0 and all mode bits are zero, the caller may disconnect after CJ and the answerer may disconnect on CJ | 8.1.2, 8.2.3 |

### 2.11 How V.92 uses V.8

- **Full Phase 1** is V.90's Phase 1 (V.92 9.1), which is a V.8 CM/JM exchange.
  - V.90 9.1.1 (1998) is the older wording of the rules in 2.6: modn0 b5 = 1 means V.90-capable, at least one pcm0 bit is set, and access0 is present.
  - V.90 9.1.1 also says that if both ends claim both analogue and digital, the **call** modem becomes the analogue modem.
  - V.92 9.1 NOTE: V.8 cannot say "V.92 only", so the V.92-or-V.90 decision waits for INFO0 in Phase 2.
- **Short Phase 1** (V.92 9.2) puts QC1a or QC1d, framed like V.8 with the Table 1 V.92 sync, **immediately before CM**. The answerer replies with QCA1x in place of JM. See section 3.
- **ODP/ADP bypass.**
  - V.92 9.3.1: if both modems indicate V.92 capability *and* both indicated LAPM **in V.8 or V.8 bis**, the V.42 ODP/ADP exchange **shall** be bypassed.
  - V.92 9.2.5: the same applies after a short Phase 1 when both QC/QCA P bits are 1.
- **Cleardown from hold by CM** (V.92 9.10.2.1).
  - A held modem that receives a CM with **no pcm0** and **every modulation bit zero** answers with a JM that also has no pcm0 and all-zero modes.
  - It then **disconnects after receiving CJ**. This hardens V.8 8.2.3's "may" into "shall".
  - A held modem that receives any other CM or a QC runs Phase 1 as the **answer** modem and ignores earlier Phase 1 information.
- **Hold and ANSam length.** A held modem sends ANSam for the whole hold timeout T1 (V.92 9.10.2.1). That overrides V.8's 5 ± 1 s. See `spec-modem-on-hold.md`.

### 2.12 Worked menus [DERIVED]

This is an analogue V.92 caller on an analogue line offering V.34, V.32 bis and V.22 bis, with LAPM. The line carries, repeated:

```
ten ONEs | E0 | C1 | 65 | 13 | 10 | 2A | 0D | 27
          sync callf0 modn0 modn1 modn2 prot0 access0 pcm0
```

| Octet | Value | Why |
|---|---|---|
| modn0 | `0x65` | `0x05`, plus PCM present, plus V.34 duplex |
| modn1 | `0x13` | extension, plus V.32 bis, plus V.22 bis |
| modn2 | `0x10` | no modes offered. The existing encoder (`Menu::octets`) always sends all three modn octets, and the V.8 extension format requires b4 = 1, so an empty `modn2` is `0x10`, not `0x00`. |
| prot0 | `0x2A` | LAPM |
| access0 | `0x0D` | no cellular, analogue |
| pcm0 | `0x27` | analogue |

- A V.92 **digital** answerer's JM repeats the call function and modes, with access0 b7 = 1 (`0x8D`) and pcm0 = `0x47`.
- The PCM category in JM means the call proceeds per V.90/V.92 (7.4).

---

## 3. V.92 short Phase 1 signals in V.8 and V.8 bis terms

### 3.1 V.8-framed QC sequences (V.92 Tables 2, 4, 11, 13; rendered pp.14, 15, 21, 22)

All four are 300 bit/s V.21 FSK in 10-bit frames. The layout is: ten ONEs, the sync `0101010101`, one framed octet, then all of that once more.

| Signal | Sender | Channel | Bits 21 / 22 | Bits 23..28 | Length | What follows |
|---|---|---|---|---|---|---|
| **QC1a** (Table 2) | analogue **caller** | V.21(L) | 21 = 0 (analogue); 22 = 0 (QC) | 23 = P; 24:29 = `W0XYZ1` | 60 bits, sent once | **immediately followed by CM** |
| **QCA1a** (Table 4) | analogue **answerer** | V.21(H) | 0 (analogue); 1 (QCA) | P; `W0XYZ1` | 70 bits (60 + ten ONEs at 60:69), once | silence |
| **QC1d** (Table 11) | digital **caller** | V.21(L) | 1 (digital); 0 (QC) | P; 24:29 = `000LM1` | 60 bits, once | **immediately followed by CM** |
| **QCA1d** (Table 13) | digital **answerer** | V.21(H) | 1 (digital); 1 (QCA) | P; `000LM1` | 70 bits, once | silence |

Bit-exact layout, with positions in time order:

```
0:9    1111111111           ten ONEs
10:19  0101010101           synchronization sequence (V.8 Table 1, "defined in V.92")
20     0                    start bit
21     A/D                  0 = analogue modem, 1 = digital modem
22     QC/QCA               0 = QC,              1 = QCA
23     P                    1 = calls for LAPM per V.42 (see 9.2.5)
24:29  analogue: W 0 X Y Z 1      digital: 0 0 0 L M 1     (bit 29 is the stop bit)
30:39  1111111111           ten ONEs
40:49  0101010101           bits 10:19 repeated
50:59  bits 20:29 repeated  (QC1a: "000PW0XYZ1", QCA1a: "001PW0XYZ1",
                             QC1d: "010P000LM1", QCA1d: "011P000LM1")
60:69  1111111111           QCA1a and QCA1d only
```

**UQTS**, the WXYZ field, is written left-most (W) first in time (Table 2):

| WXYZ | UQTS | WXYZ | UQTS |
|---|---|---|---|
| 0000 | 61 | 1000 | 75 |
| 0001 | 62 | 1001 | 78 |
| 0010 | 63 | 1010 | 79 |
| 0011 | 66 | 1011 | 82 |
| 0100 | 67 | 1100 | 83 |
| 0101 | 70 | 1101 | 86 |
| 0110 | 71 | 1110 | 87 |
| 0111 | 74 | **1111** | **Cleardown from on-hold state** |

**LM**, the level of ANSpcm (Table 11), is written L first in time:

| LM | Level |
|---|---|
| 00 | −9.5 dBm0 |
| 01 | −12 dBm0 |
| 10 | −15 dBm0 |
| 11 | −18 dBm0 |

**[DERIVED] The framed octet, in V.8 b0..b7 terms:**

- Position 21 is b0 and position 28 is b7. So QC and QCA put **b4 = 0**, as the V.8 flag-avoidance rule requires.
- Analogue: `octet = QCA<<1 | P<<2 | W<<3 | X<<5 | Y<<6 | Z<<7` (b0 = 0).
- Digital: `octet = 1 | QCA<<1 | P<<2 | L<<6 | M<<7`.
- Examples:

  | Signal | Fields | Octet |
  |---|---|---|
  | QC1a | P = 1, WXYZ = 0000 | `0x04` |
  | QC1a | P = 0, WXYZ = 0000 | **`0x00`** |
  | QC1a | P = 0, WXYZ = 0111 | **`0xE0`** |
  | QC1a | P = 1, WXYZ = 1111 (cleardown) | `0xEC` |
  | QCA1a | P = 1, WXYZ = 0000 | `0x06` |
  | QC1d | P = 1, LM = 00 | `0x05` |
  | QCA1d | P = 1, LM = 00 | `0x07` |
  | QCA1d | P = 1, LM = 11 | `0xC7` |

- These were checked against the table strings: QC1a with P = 1 and WXYZ = 0001 frames as `0001000011`, and QCA1d with P = 1 and LM = 10 frames as `0111000101`. Both match the tables.
- **Parsing hazard [DERIVED].** A QC body octet can equal the CI sync or CJ octet (`0x00`) or the CM/JM sync (`0xE0`). A receiver must therefore:
  1. recognise a QC sequence only by the `0x55` octet that comes right after an idle run of at least ten ONEs;
  2. take exactly **one** octet after it;
  3. ideally require the repeat (bits 40:59) to match.

  This is also why a V.8-only answerer is unharmed. It skips the unknown `0x55` sequence (V.8 clause 10) and goes on to the CM that follows.
- Durations at 300 bit/s [DERIVED]:

  | Signal | Duration |
  |---|---|
  | QC1a, QC1d | 200 ms, before CM starts |
  | QCA1a, QCA1d | 233.3 ms |

### 3.2 V.8 bis-framed QC messages (V.92 Tables 3, 5, 12, 14; rendered pp.15, 16, 21, 22)

- These are **V.8 bis messages**. V.92 cites the signal structure of clause 7/V.8 bis and the information-field structure of clause 8/V.8 bis. On the line, each one is:
  1. 100 ms ± 2% of V.21 mark;
  2. 2 to 5 HDLC flags;
  3. the information field;
  4. a 16-bit FCS;
  5. 1 to 3 flags.

  Zero-bit insertion applies throughout. See 4.2.
- V.92 defines only the **identification field**: 16 bits, bit position 0 first in time.

| Positions | QC2a (Table 3) | QCA2a (Table 5) | QC2d (Table 12) | QCA2d (Table 14) |
|---|---|---|---|---|
| 0:3 | `1011` message type | `1011` | `1011` | `1011` |
| 4:7 | VVVV = V.8 bis revision (NOTE: "0100" at publication; **receiver ignores**) | same | same | same |
| 8:11 | WXYZ = UQTS (Table 2) | WXYZ | 8:9 LM (Table 11), 10:11 `00` | 8:9 LM, 10:11 `00` |
| 12 | 0 reserved | 0 | 0 reserved (10:12 = `000`) | 0 |
| 13 | P (LAPM) | P | P | P |
| 14 | **0** = QC | **1** = QCA | **0** = QC | **1** = QCA |
| 15 | **0** = analogue | **0** | **1** = digital | **1** |
| Sender / channel | analogue caller, **V.21(H)** | analogue answerer, **V.21(L)** | digital caller, **V.21(H)** | digital answerer, **V.21(L)** |

**Mapping onto V.8 bis octets [DERIVED].** V.92 position k is V.8 bis octet ⌊k/8⌋+1, bit (k mod 8)+1, because bit 1 goes first (7.2.2).

- **Octet 1** is the message type in bits 1-4 plus the revision in bits 5-8.
  - Type: positions 0:3 = `1,0,1,1` give bits 1..4 = 1,0,1,1, which is **`1101`** read as bits 4..1. That is exactly V.8 bis Table 3's "Defined in ITU-T V.92". The value is `0xD`.
  - Revision: positions 4:7 = `0,1,0,0` give bits 5..8 = 0,1,0,0, which is **`0010`** read as bits 8..5. That is V.8 bis Table 4 **Revision 2**.
  - Octet 1 is therefore **`0x2D`**.
- **Octet 2:**
  - analogue: `W | X<<1 | Y<<2 | Z<<3 | P<<5 | QCA<<6` (bit 8 = 0);
  - digital: `L | M<<1 | P<<5 | QCA<<6 | 0x80`.
- These are not tree-coded 8.2 parameter blocks. The V.92 message type has its own layout, much as ACK and NAK have none (8.3.3/V.8 bis).

**Byte images [DERIVED].** The FCS is CRC-16 per ISO/IEC 3309: register preset to all ones, reflected polynomial `0x8408`, and the ones' complement sent **low octet first**. Every frame below checks to the receiver residue `0xF0B8`, which is `0001110100001111` read x^15..x^0, as V.8 bis 7.2.7 states.

| Message | Information field | FCS octets on the line |
|---|---|---|
| QC2a, WXYZ = 0000, P = 1 | `2D 20` | `0E BD` |
| QC2a, WXYZ = 0000, P = 0 | `2D 00` | `0C 9C` |
| QC2a, WXYZ = 1111 (cleardown), P = 1 | `2D 2F` | `F9 45` |
| QCA2a, WXYZ = 0000, P = 1 | `2D 60` | `0A FF` |
| QC2d, LM = 00, P = 1 | `2D A0` | `06 39` |
| QCA2d, LM = 00, P = 1 | `2D E0` | `02 7B` |
| QCA2d, LM = 01, P = 0 | `2D C2` | `12 79` |

**Channel choice [DERIVED].** V.8 bis 7.2 puts messages from the **initiating** station on V.21(L) and those from the **responding** station on V.21(H). CRe comes from the answering station, which makes the answerer the initiator (3.4/V.8 bis). So the caller's QC2x goes on V.21(H) and the answerer's QCA2x on V.21(L), which is what V.92 specifies. The V.8-framed QC1x/QCA1x follow CM/JM instead: caller on (L), answerer on (H).

**Size [DERIVED].** 100 ms of preamble plus at least 2 flags, 2 information octets, 2 FCS octets and 1 flag, before stuffing, comes to at least **≈ 287 ms**.

### 3.3 Other short Phase 1 signals (V.92 8.2.5, 8.3.1, 8.3.6)

- **TONEq**: a 980 Hz tone. [DERIVED] This is the same frequency as the V.8 bis ESi segment 2 tone.
- **QTS**: 128 repetitions of {+V, +0, +V, −V, −0, −V}, which is 768 T. V is the codeword whose Ucode is UQTS, and 0 is Ucode 0. **QTS\\** is 8 repetitions of the negated pattern (48 T). The first QTS symbol falls in data frame interval 0.
- **ANSpcm** (Table 6, rendered p.16):
  - A 301-symbol PCM sequence with a phase reversal added every 3612 symbols.
  - x = ⌊scl·√2·cos(2πk·79/301 + θ) + 0.5⌋ for k = 0..300, quantised to G.711. θ = 0.25π/301.
  - scl by level:

    | Level | µ-law scl | A-law scl |
    |---|---|---|
    | −9.5 dBm0 | 1334 | 667 |
    | −12 dBm0 | 1000 | 500 |
    | −15 dBm0 | 708 | 354 |
    | −18 dBm0 | 500 | 250 |

  - The output must equal Tables 7-10/V.92. [DERIVED] The tone is at 79/301 × 8000 ≈ 2099.7 Hz.
  - The Table 6 NOTE says some network equipment changes the channel in response to ANSpcm.
  - The digital modem announces the level it will use in the LM field.

### 3.4 Short Phase 1 procedures (V.92 9.2; rendered pp.46-50)

**Call modem, analogue (9.2.1).** It listens for ANSam and, **optionally**, CRe.

- **ANSam detected for 1 s**: send QC1a then CM, and listen for QCA1d, QCA1a and JM.
  - **QCA1d**: stop CM **without finishing the current octet**, go silent, wait for QTS, QTS\\ and ANSpcm, then go to 9.2.1.3.
  - **QCA1a**: stop CM the same way, stay silent until ANSam is heard, send TONEq, then go to 9.2.1.4.
  - **JM**: continue with V.8.
- **The first 50 ms of CRe detected**: send QC2a then silence, and listen for QCA2d, QCA2a, ANSam and ANS.
  - **QCA2d**: wait for QTS, QTS\\ and ANSpcm, then go to 9.2.1.3.
  - **QCA2a**: wait until ANSam has been heard for 1 s, send TONEq, then go to 9.2.1.4.
  - **ANSam**: go to 9.2.1.1.
  - **ANS for 3 s after QC2a**: continue with V.8.
  - **Neither QCA2x within 1 s after QC2a**: continue with V.8 bis.
- **9.2.1.3.** After ANSpcm has been heard for 1 s, send TONEq for at least 50 ms. If ANSam was already heard for 1 s, TONEq may start as soon as ANSpcm is detected. When ANSpcm stops, end TONEq, stay silent **75 ± 5 ms**, then start **Phase 2**.
- **9.2.1.4.** When ANSam stops, end TONEq, stay silent 75 ± 5 ms, then start **V.34 Phase 2**.

**Call modem, digital (9.2.2).** It listens for ANSam and, optionally, CRe.

- **ANSam for 1 s**: send QC1d then CM, and listen for QCA1a, JM and ANSam.
  - **QCA1a**: stop CM mid-octet, stay silent 75 ± 5 ms, send QTS, QTS\\ and ANSpcm, then go to 9.2.2.3.
  - **ANSam for 1 s after QC1d, or JM**: continue with V.8.
- **The first 50 ms of CRe**: send QC2d then silence, and listen for QCA2a, ANSam and ANS.
  - **QCA2a**: stay silent 75 ± 5 ms, send QTS, QTS\\ and ANSpcm, then go to 9.2.2.3.
  - **ANSam**: go to 9.2.2.1.
  - **ANS for 3 s after QC2d**: continue with V.8.
  - **No QCA2a within 1 s**: continue with V.8 bis. The text says "after transmitting QC2a" here; see §8.
- **9.2.2.3.** While sending ANSpcm, listen for TONEq. When it is heard, stay silent 75 ± 5 ms, then start Phase 2.

**Answer modem, analogue (9.2.3).**

- On connection it stays silent for **at least 200 ms**, then sends ANSam (V.8) or CRe (V.8 bis).
- **Sending ANSam** (9.2.3.1). This also applies after an earlier V.8 bis session timed out. It listens for QC1d, QC1a and CM.
  - **QC1d**: send QCA1a then silence, wait for QTS, QTS\\ and ANSpcm, then go to 9.2.3.3.
  - **QC1a**: it *may* send QCA1a, stay silent 75 ± 5 ms, send ANSam, then go to 9.2.3.4.
  - **CM**: continue with V.8.
- **Sending CRe** (9.2.3.2). It listens for QC2d and V.8 bis signals.
  - **QC2d**: stop CRe, send QCA2a then silence, wait for QTS, QTS\\ and ANSpcm, then go to 9.2.3.3.
  - **QC2a**: it *may* send QCA2a, stay silent 75 ± 5 ms, send ANSam, then go to 9.2.3.4.
  - **Any other V.8 bis signal**: continue with V.8 bis.
  - **Nothing within 3 s after CRe**: send ANSam, then go to 9.2.3.1.
- **9.2.3.3.** After ANSpcm has been heard for 1 s, send TONEq for at least 50 ms. If ANSam was sent in 9.2.3.1, TONEq may start at ANSpcm detection. When ANSpcm stops, end TONEq, stay silent 75 ± 5 ms, then start Phase 2.
  - **No ANSpcm within 2 s after QCA1a**: send ANSam and continue with V.8.
  - **No ANSpcm within 2 s after QCA2a**: send ANSam, then go to 9.2.3.1.
- **9.2.3.4.** While sending ANSam, listen for TONEq and CM.
  - **CM**: continue with V.8.
  - **TONEq**: end ANSam, stay silent 75 ± 5 ms, then start Phase 2.

**Answer modem, digital (9.2.4).** It uses the same 200 ms silence, then sends ANSam or CRe.

- **Sending ANSam** (9.2.4.1). It listens for QC1a, QC1d and CM.
  - **QC1a**: send QCA1d, stay silent 75 ± 5 ms, send QTS, QTS\\ and ANSpcm, then go to 9.2.4.3.
  - **QC1d**: it *may* take the analogue role and go to 9.2.3.1.
  - **CM**: continue with V.8.
- **Sending CRe** (9.2.4.2). It listens for QC2a, QC2d and V.8 bis signals.
  - **QC2a**: stop CRe, send QCA2d, stay silent 75 ± 5 ms, send QTS, QTS\\ and ANSpcm, then go to 9.2.4.3.
  - **QC2d**: it *may* take the analogue role and go to 9.2.3.2.
  - **Any other V.8 bis signal**: continue with V.8 bis.
  - **Nothing within 3 s after CRe**: send ANSam, then go to 9.2.4.1.
- **9.2.4.3.** While sending ANSpcm, listen for TONEq.
  - **TONEq**: stay silent 75 ± 5 ms, then start Phase 2.
  - **No TONEq within 2 s after QCA1d**: send ANSam and continue with V.8.
  - **No TONEq within 2 s after QCA2d**: send ANSam, then go to 9.2.4.1.

**Figures 3-8**, read from the renders on pp.46-48:

| Figure | Scenario | Timeline |
|---|---|---|
| 3 | analogue caller, answerer sends ANSam | caller: 1 s, QC1a, CM … TONEq (starts ≤ 1 s after ANSpcm starts, ends after ANSpcm). Digital answerer: ANSam, QCA1d, 75 ± 5 ms, QTS (768T), QTS\\ (48T), ANSpcm. |
| 4 | digital caller, answerer sends ANSam | analogue answerer: ANSam, QCA1a … TONEq (≤ 1 s). Digital caller: 1 s, QC1d, CM, 75 ± 5 ms, QTS, QTS\\, ANSpcm. |
| 5 | analogue caller, answerer sends CRe | caller: QC2a ≥ 50 ms after CRe starts … TONEq (1 s). Digital answerer: CRe, QCA2d, 75 ± 5 ms, QTS, QTS\\, ANSpcm. |
| 6 | digital caller, answerer sends CRe | analogue answerer: CRe, QCA2a … TONEq (1 s). Digital caller: QC2d at ≥ 50 ms, then 75 ± 5 ms, QTS, QTS\\, ANSpcm. |
| 7 | both analogue, ANSam | caller: 1 s, QC1a, CM … TONEq, 75 ± 5 ms, V.34 Phase 2. Answerer: ANSam, QCA1a, 75 ± 5 ms, ANSam, 75 ± 5 ms, V.34 Phase 2. |
| 8 | both analogue, CRe | caller: QC2a at ≥ 50 ms; after 1 s of ANSam, TONEq, 75 ± 5 ms, V.34 Phase 2. Answerer: CRe, QCA2a, 75 ± 5 ms, ANSam, 75 ± 5 ms, V.34 Phase 2. |

[DERIVED] In Figures 5 and 6, QC2x starts while CRe is still sounding. The CRe sender must therefore detect a V.21 message against its own CRe echo.

---

## 4. ITU-T V.8 bis (11/2000)

### 4.1 Signals (7.1; rendered pp.13-14)

- The tone signals are MRe, MRd, CRe, CRd, ESi and ESr. Each is **segment 1** (a dual tone) followed by **segment 2** (a single tone).
- The subscript "e" means sent by an automatic answerer at call establishment. The subscript "d" means sent during telephony.

| Signal | Segment 1 (dual tone) | Segment 2 |
|---|---|---|
| MRe | 1375 + 2002 Hz (initiating) | 650 Hz |
| MRd (initiating) | 1375 + 2002 Hz | 1150 Hz |
| MRd (responding) | 1529 + 2225 Hz | 1150 Hz |
| **CRe** | **1375 + 2002 Hz** | **400 Hz** |
| CRd (initiating) | 1375 + 2002 Hz | 1900 Hz |
| CRd (responding) | 1529 + 2225 Hz | 1900 Hz |
| ESi | 1375 + 2002 Hz | 980 Hz |
| ESr | 1529 + 2225 Hz | 1650 Hz |

- **Durations** (7.1.2):
  - segment 1 is **400 ms**, but MRe and CRe may shorten it to **285 ms** for compatibility with modems that lack V.8 bis;
  - segment 2 is **100 ms**;
  - the NOTE advises keeping 400 ms where echo suppressors may be present.
- **Tolerances** (7.1.3): frequency ±250 ppm; duration ±2%.
- **Power** (7.1.4): per V.2. CRe and MRe must be **12 to 15 dB below** nominal continuous power. The other signals should preferably be at the highest permitted level.
- The receiver tells signals apart by the **segment 2 frequency** (10.1.3, 10.2.1).
- V.92 acts after detecting **the initial 50 ms of CRe** (9.2.1.2, 9.2.2.2). [DERIVED] At that point only segment 1 (1375 + 2002 Hz) has been heard, and it is shared with MRe, CRd and ESi. The V.92 caller therefore commits before segment 2 identifies the signal. See §9.

### 4.2 Messages and framing (7.2; rendered pp.14-16)

- **Messages** are MS, CL, CLR, ACK(1), ACK(2), NAK(1) to NAK(4), and the V.92 type. All use V.21 FSK: **V.21(L)** from the initiating station, **V.21(H)** from the responding station. F_A and F_Z are held to ±0.01%.
- **Format** (7.2.2):
  - Octets are sent in ascending order, and **bit 1 of each octet first**.
  - In a single-octet field, the lowest-numbered bit is the LSB.
  - A multi-octet field is shown in Figure 2: the LSB (2^0) is the lowest-numbered bit of the **highest-numbered** octet, and significance rises as octet number falls.
  - **The FCS is the exception** (Figure 3). Bit 1 of the first FCS octet is 2^15, bit 8 of the first octet is 2^8, bit 1 of the second octet is 2^7, and bit 8 of the second octet is 2^0. This is ordinary HDLC FCS ordering.
- **Frame** (Figure 4, 7.2.3 and 7.2.5):
  1. 2 flags, plus up to 3 optional flags (**2 to 5 opening flags**);
  2. information field;
  3. FCS, 2 octets;
  4. 1 flag, plus up to 2 optional flags (**1 to 3 closing flags**).

  Flags are `01111110`.
- **Preamble** (7.2.4): **100 ms ± 2%** of continuous V.21 mark before every message. When ES precedes the message, that 100 ms counts as ES segment 2.
- **FCS** (7.2.7):
  - It is 16 bits per ISO/IEC 3309, generator x^16 + x^12 + x^5 + 1.
  - It covers everything between the last opening flag and the FCS, excluding stuffed zeros.
  - The transmitter presets the register to all ones and sends the ones' complement of the remainder.
  - A receiver that also presets to ones sees a remainder of **`0001110100001111`** (x^15..x^0) on a good frame.
  - This is identical to `crates/ec/src/hdlc.rs`: `Fcs`, `GOOD = 0xf0b8`, reflected `0x8408`.
- **Transparency** (7.2.8): insert a 0 after five consecutive 1s between the flags, and remove it on receive.
- **Invalid frame** (7.2.9), any one of:
  - not bounded by flags per 7.2.5;
  - **fewer than 3 octets** between the flags;
  - not a whole number of octets;
  - FCS error.

  On an invalid frame, send **NAK(1)** and return to the Initial V.8 bis State (9.8).
- **Timeout** (9.8): more than **5 s** in any state other than telephony or MS mode means return to the Initial State.
- **Segmentation** (9.10): an information field may hold **at most 64 octets**. More needs "additional information available" plus ACK(2).

### 4.3 Information field (8.1, 8.2; rendered pp.18-19)

- **Layout**: Identification field (I), then Standard information field (S), then an optional Non-standard field (NS).
- **Tree coding** (8.2) is used for the I parameter field and for S:
  - The order on the line is: **{NPar(1)} block, {SPar(1)} block, then one Par(2) block for each SPar(1) bit set**, in order.
  - Each Par(2) block is {NPar(2)}, then {SPar(2)}, then one {NPar(3)} block for each SPar(2) bit set.
  - Transmission ends with the last octet of Par(2)_N (Figure 7).
- **Delimiting** (8.2.3). In every octet, a delimiter bit of **0 means more octets follow** in the block, and **1 means this is the last octet**.
  - **Bit 8** delimits the {NPar(1)} block, the {SPar(1)} block, and each Par(2) block.
  - **Bit 7** delimits each {NPar(2)} block, each {SPar(2)} block, and each {NPar(3)} block.
  - A Par(2) block made only of NPar(2) octets has **bits 7 and 8 both set** in its last NPar(2) octet.
  - Level-1 parameters use bits 1-7. Level-2 parameters use bits 1-6.
- Receivers must parse every block and ignore what they do not understand.

### 4.4 Identification field (8.3; rendered pp.20-22)

**Octet 1** holds the message type in bits 1-4 (Table 3) and the revision in bits 5-8 (Table 4). Tree rules do not apply to octet 1.

| Message type | Bits 4 3 2 1 | [DERIVED] octet 1 with revision 2 |
|---|---|---|
| MS | 0 0 0 1 | `0x21` |
| CL | 0 0 1 0 | `0x22` |
| CLR | 0 0 1 1 | `0x23` |
| ACK(1) | 0 1 0 0 | `0x24` |
| ACK(2) | 0 1 0 1 | `0x25` |
| NAK(1) | 1 0 0 0 | `0x28` |
| NAK(2) | 1 0 0 1 | `0x29` |
| NAK(3) | 1 0 1 0 | `0x2A` |
| NAK(4) | 1 0 1 1 | `0x2B` |
| **Defined in V.92** | **1 1 0 1** | **`0x2D`** |

- Other message-type code points are reserved. A receiver parses the field and ignores what it does not understand.
- Revision (Table 4, bits 8 7 6 5): Revision 1 = `0 0 0 1`, **Revision 2 = `0 0 1 0`**. This 2000 edition is Revision 2.
- The parameter field (8.3.3) is tree-coded for **CL, CLR and MS**, and has **zero length for ACK and NAK**. S is likewise zero length for ACK and NAK (8.4.1).

**I-field NPar(1)** (Table 5-1). Bit 8 is the delimiter.

| Bit | Meaning |
|---|---|
| 1 | V.8 start-up (9.9.1) |
| 2 | Short V.8 start-up (9.9.2) |
| 3 | Additional information available (9.10) |
| 4 | Transmit ACK(1) (9.7) |
| 5, 6 | reserved |
| 7 | non-standard field |

All of bits 7..1 at zero means no parameters.

**I-field SPar(1)** (Table 5-2):

- Bit 1 = **network type**. Its NOTE: when this bit is absent, the DCE is on an analogue PSTN connection.
- Bits 2-7 are reserved.

**Network type NPar(2)** (Table 5-3). Bits 7 and 8 are delimiters.

| Bit | Meaning |
|---|---|
| 1 | cellular access |
| 2 | ISDN access |
| 3 | **digital PSTN access** (NOTE: digital access other than ISDN, where the DCE delivers digitally encoded analogue content to the network) |
| 4, 5 | reserved |
| 6 | non-standard network |

### 4.5 Standard information field: the PCM bits (8.4; rendered pp.22, 24, 25)

- S NPar(1) (Table 6-1): bits 1-6 are reserved and bit 7 is non-standard capabilities.
- S SPar(1), octet 1 (Table 6-2a):

  | Bit | Meaning |
  |---|---|
  | 1 | **Data** |
  | 2 | simultaneous voice and data (SVD) |
  | 3 | H.324 |
  | 4 | V.18 |
  | 5 | T.30 fax |
  | 6 | analogue telephony |
  | 7 | T.101 |

- S SPar(1), octet 2 (Table 6-2b): bit 1 = H.324-Multilink, bit 2 = Multilink-Additional-Connection, others reserved.
- For CL and CLR, several set bits mean "all of these". For **MS**, set several bits only if they can all run at once (8.4).

**Data NPar(2)**: four octets. Bits 7 and 8 are delimiters. No data SPar(2) or NPar(3) are defined.

| Octet | Bit 1 | Bit 2 | Bit 3 | Bit 4 | Bit 5 | Bit 6 |
|---|---|---|---|---|---|---|
| 1 (6-3a) | transparent data | **V.42** error control | V.42 bis | V.14 | T.120 | non-standard |
| 2 (6-3b) | T.84 SPIFF | T.434 | V.80 sync HDLC | reserved | V.34 (duplex) | V.32 bis |
| 3 (6-3c) | V.32 | V.22 bis | V.22 | V.21 | **V.90 analogue** | **V.90 digital** |
| 4 (6-3d) | V.91 | **V.92 analogue** | **V.92 digital** | reserved | reserved | reserved |

- The NOTE to 6-3c and 6-3d says a **digital** V.90/V.92 modem cannot work on an analogue PSTN connection (see Table 5-2).
- SVD NPar(2) (6-4a and 6-4b) has its own V.34 and V.32 bis bits. They are not relevant here.

**[DERIVED] Example CL for "V.8 start-up, data, V.42, V.92 analogue", with nothing else:**

```
I:  22            CL, revision 2
    81            NPar(1): V.8 start-up (bit 1), last (bit 8)
    80            SPar(1): nothing, last  (so no network-type Par(2))
S:  80            NPar(1): nothing, last
    81            SPar(1) octet 1: Data, last
    02 00 00 C2   Data NPar(2) octets 1-4: V.42; -; -; V.92 analogue + bit7 + bit8
```

- **LAPM in V.8 bis**, as 9.3.1/V.92 uses the term, is Data NPar(2) octet 1 **bit 2**.
- In QC2x/QCA2x it is the **P** bit instead.

### 4.6 Procedures that bear on V.92 (9.7-10.2; rendered pp.35-40)

- **9.7.** An MS with "transmit ACK(1)" set is acknowledged with ACK(1), and **then** ANS or ANSam follows at once. With the bit clear, ACK(1) is left out. A NAK is always sent when rejecting.
- **9.9.** After an MS that selects a modem mode, the station that **received MS becomes the answer modem**, whichever end placed the call.
  - **9.9.1, V.8 start-up** (Figure 13). The sequence is MS, ACK, ANSam, then after Te the CM, JM, CJ, sigC/sigA. Parameters in CM and JM override those in MS.
  - **9.9.2, short V.8** (Figure 14). This is the recommended start for V.34 and later modems.
    1. B sends ANSam for a fixed Te.
    2. B sends JM showing the **single** mode chosen in MS.
    3. A sends **no CM**. It waits for two identical JMs, then sends CJ.
  - **9.9.3, V.25 start-up** (Figure 15). This applies when both the V.8 and the short V.8 bits are 0. The sequence is ACK, then ANS, then the modem's own start-up.
- **10.2.1, calling station.**
  - It enters the Initial State when it goes off-hook, and listens for MRe, MRd, CRe, CRd and ESi as well as ANS and ANSam.
  - If ANS or ANSam comes first, it leaves V.8 bis and runs V.25 or V.8.
  - On **MRe or CRe** it sends MRd or CRd, **or an appropriate message response preceded by ESr**. V.92 replaces that response with QC2x (see §9 about ESr).
  - It must cope with two initiating signals less than about 0.5 s apart.
- **10.2.2, answering station (CRe or MRe).**
  - It enters the Initial State and stays silent for **at least 400 ms**.
  - It sends MRe or CRe and listens for MRd, CRd and ESr. An OGM (outgoing message) may follow if telephony is available.
  - With no V.8 bis signal **within 3 s** and no OGM, it may resend or clear down.
  - It may listen for CNG, CT or CI, and then send ANS or ANSam.
  - Energy within 1.5 s may prompt it to resend CRe or MRe at a higher level.
- **Clause 11**: the DTE-DCE side of V.8 bis is V.251, which is not in `docs/specs`.

### 4.7 Does V.92 short Phase 1 depend on V.8 bis?

**No.**

- V.92 9.2.1 and 9.2.2 require callers to detect ANSam, and to detect CRe only **optionally**.
- 9.2.3 and 9.2.4 let answerers send **ANSam or CRe**.
- The complete short Phase 1 with QC1a/QCA1a or QC1d/QCA1d needs only:
  - V.8 ANSam and CM/JM machinery;
  - the V.8-framed QC octets (3.1);
  - TONEq, QTS and ANSpcm.

V.8 bis matters only in these cases:

1. **Our answerer chooses to send CRe.** Then we need CRe generation, V.8 bis message **reception** on V.21(H) for QC2x, and message **transmission** on V.21(L) for QCA2x. We also need an ordinary V.8 bis fallback, or we can let the 3 s timer take us to ANSam.
2. **Our caller wants the CRe path.** Then we need CRe detection (at least the 50 ms of segment 1) and a V.8 bis message sender on V.21(H) and receiver on V.21(L).
3. A digital answerer that sends CRe to a caller that does not detect CRe loses **3 s** (9.2.4.2) and then runs 9.2.4.1 with ANSam as usual.

**Recommendation [DERIVED]:** implement the V.8 path first. Add the CRe path only if a real capture shows a server sending CRe. The memory notes on live V.90 servers record ANSam only; no capture with CRe is known.

---

## 5. ITU-T V.250 (07/2003)

### 5.1 Clause 6.8, PCM DCE commands (rendered pp.91-97; Table I.2 on p.100)

6.8 introduces the `+P` family as the commands that condition and control a DCE's use of V.92. Table I.1 lists `+P` as "PCM DCE commands, ITU-T Rec. V.92".

| Command | Table I.2 type | Syntax as printed | Values | Default | Read / test replies | Mandatory |
|---|---|---|---|---|---|---|
| `+PCW` (6.8.1) Call Waiting enable | Parameter | `+PCW=[<call waiting>]` | 0, 1, 2 (Table 31) | **0** | `+PCW: <call waiting>`; test `+PCW: (0,1,2)` | if V.92 is implemented |
| `+PMH` (6.8.2) Modem-on-Hold enable | Parameter | `+PMH=[<value>]` | 0, 1 (Table 32) | **0** | `+PMH: <current setting>`; test `+PMH: (0,1)` | if V.92 |
| `+PMHT` (6.8.3) Modem-on-Hold Timer | Parameter | **no syntax line printed** | 0-13 (Table 33) | **not stated** | `+PMHT: <current setting>`; test `+PMHT: (0,1,2,3,4,5,6,7,8,9,10,11,12,13)` | if V.92 |
| `+PMHR` (6.8.4) Initiate Modem on Hold | **Action** (the heading says "Parameter") | `+PMHR` ("Read Syntax: +PMHR") | none defined | n/a | reply `+PMHR: <value>` (Table 34) | if V.92 |
| `+PIG` (6.8.5) PCM upstream ignore | Parameter | `+PIG=[<value>]` | 0, 1 (Table 35) | **0** | `+PIG: <current setting>`; test `+PIG: (0,1)` | if V.92 |
| `+PMHF` (6.8.6) V.92 Modem-on-Hold Hook Flash | Parameter (in effect an action) | `+PMHF` | none | n/a | no read or test syntax given | if V.92 |
| `+PQC` (6.8.7) V.92 Phase 1 and Phase 2 Control | Parameter | `+PQC=<value>` | 0-3 (Table 36) | **0** | `+PQC: <current setting>`; test `+PQC: (0,1,2,3)` | if V.92 |
| `+PSS` (6.8.8) Use Short Sequence | Parameter | `+PSS=<value>` | 0-2 (Table 37) | **0** | `+PSS: <current setting>`; test `+PSS: (0,1,2)` (printed under "Text syntax") | if V.92 |

V.250 (07/2003) defines no other `+P` commands.

#### Table 31: `+PCW <call waiting>`

What the DCE does when it detects call waiting during V.92 operation:

| Value | Action |
|---|---|
| 0 | Toggle V.24 **circuit 125** and collect Caller ID if **+VCID** has enabled it |
| 1 | Hang up |
| 2 | Ignore V.92 call waiting |

`+VCID` belongs to V.253, which is not in `docs/specs`.

#### Table 32: `+PMH`

The sense is inverted, so check it carefully.

| Value | Meaning |
|---|---|
| **0** | **Enables** V.92 modem-on-hold |
| **1** | **Disables** V.92 modem-on-hold |

#### Table 33: `+PMHT`

Whether this DCE **grants or denies** a hold request **from the far end**, and with what timeout.

| Value | Meaning | Value | Meaning |
|---|---|---|---|
| 0 | Deny V.92 MOH request | 7 | Grant, 3 min |
| 1 | Grant, 10 s | 8 | Grant, 4 min |
| 2 | Grant, 20 s | 9 | Grant, 6 min |
| 3 | Grant, 30 s | 10 | Grant, 8 min |
| 4 | Grant, 40 s | 11 | Grant, 12 min |
| 5 | Grant, 1 min | 12 | Grant, 16 min |
| 6 | Grant, 2 min | 13 | Grant, **indefinite** |

The example in the text shows `+PMHT: 0` for "deny", but no default is given.

#### Table 34: `+PMHR` responses

- The command asks the DCE to **start or confirm** a hold procedure.
- It returns **ERROR** if hold is not enabled or the DCE is idle.
- Otherwise it returns `+PMHR: <value>`. The value is the timer received, or the request status. The reply **may be delayed**, depending on whether the command answers an incoming request or starts one.

| Value | Meaning |
|---|---|
| 0 | V.92 MOH request denied or not available; the modem may try again later |
| 1 … 13 | MOH granted with timeout 10 s, 20 s, 30 s, 40 s, 1, 2, 3, 4, 6, 8, 12, 16 min, indefinite (same order as Table 33) |
| 14 | MOH request denied, **and future requests in this session will also be denied** |

#### Table 35: `+PIG`

| Value | Meaning |
|---|---|
| 0 | Enable PCM upstream |
| 1 | Disable PCM upstream |

#### `+PMHF` (6.8.6)

- The DCE goes on-hook for a set time, normally **half a second** unless national rules say otherwise, and then comes back. The text literally says "return on-hook"; see §8.
- It returns **ERROR if the modem is not on hold**.
- It applies only to V.92 modem-on-hold.
- Compare the dial modifier `!` (6.3.1.5, register recall), which goes on-hook for a set time, normally half a second, and returns **off-hook**.

#### Table 36: `+PQC`

- Enables or disables the V.92 shortened Phase 1 and Phase 2 procedures **globally**. It does not start them.
- It works together with `+PSS`.

| Value | Meaning |
|---|---|
| 0 | Enable short Phase 1 **and** short Phase 2 |
| 1 | Enable short Phase 1 |
| 2 | Enable short Phase 2 |
| 3 | Disable short Phase 1 and short Phase 2 |

#### Table 37: `+PSS`

A **calling** DCE uses this to force a short or full start-up on the **next and later** connections.

| Value | Meaning |
|---|---|
| 0 | The DCEs decide whether to use short start-up. Short procedures run only if `+PQC` enables them. |
| 1 | Force short start-up on the next and later connections, **if +PQC enables them** |
| 2 | Force full start-up on the next and later connections, **regardless of +PQC** |

### 5.2 Other V.250 text that concerns V.92

- **`H` (6.3.6), NOTE.** With V.92 modem-on-hold, `H` may end the **call** without taking the line on-hook. Commands such as `AT+PMHF` can then make the PSTN switch to another line, to place an outgoing call or take an incoming one.
  - `H0` is defined.
  - Result: `OK` after circuit 109 turns off, or `ERROR` for an unsupported value.
- **`O` (6.3.7).** `O0` returns to online data state. It is **also used to retrain after a modem-on-hold transaction, or to reconnect to a modem that has been put on hold per V.92.** Results (Table 10):

  | Result | When |
  |---|---|
  | `CONNECT` | resumed, with X0 |
  | `CONNECT <text>` | resumed, with Xn (n ≠ 0) |
  | `NO CARRIER` | not resumed |
  | `ERROR` | unsupported value |

- **Circuit 125 and RING** (4.1, 6.3.4).
  - The DCE may intercept circuit 125 (calling indicator) to detect alerting and auto-answer.
  - `RING` (numeric 2) is the only unsolicited result code in 6.3. It is repeated each time the network repeats its alerting indication.
  - **No V.250 result code is defined for a V.92 call-waiting event.** `+PCW=0` speaks only of toggling circuit 125.
- **`+MS` (6.4.1; Table 13 rendered p.56).**
  - Syntax: `+MS=[<carrier>[,<automode>[,<min_rate>[,<max_rate>[,<min_rx_rate>[,<max_rx_rate>]]]]]]`.
  - **`V92` = ITU-T V.92** (alongside `V90` and `V91`). Proprietary carrier strings must not start with "V".
  - `<automode>`: 0 = disabled; 1 = enabled, "with V.8 or Annex A/V.32 bis where applicable".
  - Rates are in bit/s. **0 means "determined by the carrier and automode"**. The `_rx_` pair lets the receive direction have its own limits, which suits asymmetric modes.
  - Recommended defaults:

    | Subparameter | Default |
    |---|---|
    | carrier | manufacturer-specific |
    | automode | 1 (if possible) |
    | min_rate | 0 |
    | max_rate | 0 (the maximum the carrier supports) |
    | min_rx_rate | 0, if implemented |
    | max_rx_rate | 0, if implemented |

  - Read reply: `+MS: <carrier>,<automode>,<min_rate>,<max_rate>,<min_rx_rate>,<max_rx_rate>`. Optional subparameters may be left out if unimplemented or 0.
  - Test reply: lists of supported values. The printed example is `+MS: (V21,V22,V22B,V32,V32B),(0,1),(0,300-14400),(0,300-14400)`.
  - [DERIVED] V.92 rate ranges from V.92 clause 1:
    - downstream 28 000 to 56 000 bit/s in steps of 8000/6;
    - upstream 24 000 to 48 000 bit/s in steps of 8000/6, or V.34 rates.
- **`+MA` (6.4.2)** is optional. Changing `+MS=<carrier>` resets it.
- **`+MR` (6.4.3)** controls the intermediate result codes `+MCR: <carrier>` (for example `+MCR: V92`) and `+MRR: <rate>[,<rx_rate>]`.
  - They are sent once the modulation and rate are decided, before `+ER`, `+DR` and `CONNECT`.
  - They report the current modulation, **negotiated or renegotiated**.
  - `<rate>` is the transmit rate, or 0 if negotiation failed. `<rx_rate>` may be added when the receive rate differs, which is always so for V.92.
  - Values: 0 = off (recommended default), 1 = on.
- **`+ES` (6.5.1; Table 20, p.66).** `<orig_rqst>`:

  | Value | Meaning |
  |---|---|
  | 0 | direct mode |
  | 1 | buffered only |
  | **2** | V.42 **without the detection phase**; when V.8 is in use, a request to disable the V.42 detection phase |
  | 3 | V.42 with the detection phase |
  | 4 | Alternative protocol |

  This ties to prot0 and V.92 9.3.1.
- **`+DS44` (6.6.2)** and **`+DR` (6.6.3, Table 29, p.78).**
  - Report strings: `+DR: NONE`, `V42B`, `V42B RD`, `V42B TD`, **`V44`**, **`V44 RD`**, **`V44 TD`**.
  - `+DR` comes after `+ER` and before `CONNECT`. `+DR` values: 0 = off (default), 1 = on.
  - `+DS44` is mandatory if V.44 is implemented. It is already implemented in `crates/at`.
- **`+GCAP` (6.1.9).** Table 2 has no `+P` entry, so V.250 does not require a `+GCAP` token for V.92.
- **`+TMO` (6.9; rendered pp.97-99).** This retrieves V.59 managed objects.

  | Form | Meaning |
  |---|---|
  | `+TMO` | repeat the last `+TMO` |
  | `+TMO [<list level><n>]=?` | list supported objects |
  | `+TMO <tagID or Name> <all or only>` | fetch one object |

  - List levels: 0 = all objects, 1 = high-level, 2 = mid-level, 3 = low-level, 4 = report 0 if names are supported and 1 if tagIDs are.
  - `n`, if present, asks for names. It must not be used with level 4. ERROR if only tagIDs are supported.
  - The reply takes the same form as the request.
  - A 2-digit tagID means a high-level object. A 4-digit tagID means a mid- or low-level object.
  - Examples: `+TMO V92 All`, `+TMO V92 rxHistory`, `+TMO 09` (the whole V.90 object), `+TMO 0900`.
  - NOTE: for shared mid-level objects, only the one for the most recent modulation is returned.
  - **The V.59 object definitions are not in `docs/specs`,** so V.92 diagnostics cannot be built from the specs we hold.

### 5.3 How the `+P` settings map onto V.92 signalling [DERIVED]

V.250 names only behaviour. The links below to V.92 fields are this digest's reading. They are consistent with the tables, but neither Recommendation states them.

| Setting | V.92 effect |
|---|---|
| `+PMH=1` | Do not start modem-on-hold. When MHreq arrives, answer **MHnack** (V.92 9.10.2.1 lets a modem deny with MHnack). `+PMHR` returns ERROR (6.8.4). |
| `+PMHT=0` | When MHreq arrives, answer MHnack. |
| `+PMHT=n` (1 ≤ n ≤ 13) | When MHreq arrives, answer **MHack** with T1 code = **n as a 4-bit number, left bit first in time** (Table 33/V.92: `0001` = 10 s … `1100` = 16 min, `1101` = no limit). The two tables list the same 13 timeouts in the same order. |
| `+PMHR` as initiator | Send **MHreq** (after Tone RT or an MH response; 9.10.1). |
| `+PMHR` reply to MHack | `+PMHR: <T1 code as a number>` (1-13). |
| `+PMHR` reply to MHnack | `+PMHR: 0`. The initiator must then send MHcda or MHfrr **within 10 s** (9.10.2.1), so the DCE has to choose one; V.250 gives no command to steer that. |
| `+PMHR: 14` | Local policy. No V.92 MH code means "never again". |
| `+PMHR` as responder | "Confirm" an incoming request. This fits a DCE that waits for the DTE before granting. The auto-grant setting in `+PMHT` suggests many DCEs answer by themselves. |
| `+PCW=0` | Tell the DTE (circuit 125), then either hold (MHreq) or clear down. |
| `+PCW=1` | Clear down. A graceful path is **MHclrd** with reason `0101` "cleardown due to incoming call" (Table 32/V.92), then disconnect on MHcda. |
| `+PCW=2` | Keep the data call and ignore the waiting call. |
| `+PMHF` | While held, flash the hook to reach the waiting call, or to get dial tone for an outgoing call. MHclrd reason `0110` is "cleardown due to outgoing call". |
| `ATO` after hold | The returning modem is the **call** modem and runs **short Phase 1** (QC1a/QC1d after 1 s of ANSam), as `spec-modem-on-hold.md` §6.3 describes. Then comes Phase 2 and, when both are V.92, short Phase 2. |
| `ATH` while held | End the call without going on-hook. |
| `+PIG=1` | Do not offer or select PCM upstream. The analogue modem does not choose the Table 18 INFO1a (PCM upstream) layout in 9.3/9.4. On the digital side it would mean refusing PCM upstream reception. This digest has not established which V.92 field carries that refusal (§9 item 12); see `spec-phase2-signals.md` and `spec-phase4-*.md`. |
| `+PQC` = 0 or 1 | Short Phase 1 is allowed. As caller: send QC1x/QC2x. As answerer: answer QC1x/QC2x with QCA. |
| `+PQC` = 2 or 3 | Short Phase 1 is off. As caller: send CM only. As answerer: treat QC as absent and wait for CM, which QC1x always precedes. |
| `+PQC` = 0 or 2 | Short Phase 2 is allowed. Set the request bit: **INFO0a bit 27** (analogue) or **INFO0d bit 26** (digital), V.92 9.4. |
| `+PQC` = 1 or 3 | Clear that request bit. |
| `+PSS=0` | Send QC1x only when the connection is "recognised", i.e. a stored UQTS (analogue) or LM (digital) exists for this number. V.92 scope j) is the only hint; the criterion is left to the implementation. |
| `+PSS=1` | Always send QC when `+PQC` allows it. |
| `+PSS=2` | Never send QC or request short Phase 2. |

### 5.4 A plausible DTE flow [DERIVED, informative only]

1. Online. Call waiting is detected and `+PCW=0` is set, so circuit 125 toggles and Caller ID is collected.
2. The DTE escapes (`+++`) and sends `AT+PMHR`. The reply is `+PMHR: 5`, meaning hold granted for 1 min.
3. `AT+PMHF` flashes the hook to the waiting call.
4. When done, `AT+PMHF` flashes back.
5. `ATO` runs short Phase 1 as caller and ends with `CONNECT`, or with `NO CARRIER` if the far end timed out.
6. Alternatively, `ATH` ends the data call without going on-hook.

---

## 6. V.92 and V.42 / V.44

### 6.1 What V.92 itself says

- **7.2.** An asynchronous-to-synchronous converter at the DTE side follows **V.14, V.42 or V.80**, and "data compression may also be employed".
- **V.92 never names V.44.** V.44 (11/2000) never names V.92, V.90 or V.8. The link is only practical:
  - both are 2000-era V.42 add-ons;
  - V.250 (2003) provides `+DS44` and `+DR: V44`;
  - V.42 (2002) carries the V.44 XID parameters.
- References: V.92 cites **V.42 (1996)**. The 2002 V.42 in `docs/specs` is the one that mentions V.92.

### 6.2 Negotiating LAPM without ODP/ADP

| Where LAPM is indicated | Bit |
|---|---|
| V.8 CM/JM | prot0 = `0x2A` (Table 6) |
| V.92 QC1x/QCA1x | bit 23 = **P** |
| V.92 QC2x/QCA2x | ID-field bit 13 = **P** |
| V.8 bis CL/CLR/MS | Data NPar(2) octet 1, **bit 2** (V.42) |

The bypass rules:

- **V.92 9.2.5.** After a **short** Phase 1, if both modems indicated LAPM, the V.42 ODP/ADP exchange **shall be bypassed**.
- **V.92 9.3.1.** After a **full** Phase 1, if both modems indicate **V.92 capability** (INFO0d bit 27 and INFO0a bit 26) and both indicated LAPM in V.8 or V.8 bis, the exchange **shall be bypassed**.
  - [DERIVED] The decision therefore waits for INFO0 in Phase 2.
  - If one end is only V.90, V.8 7.3 and 7.4 say only "may be required". Keep today's behaviour: run the detection phase, and use prot0 as a hint.
- **V.42 7.2.1.2.** The originator's detection phase "may be disabled by the user". The originator then goes straight to protocol establishment.
- **V.250 `+ES` orig_rqst = 2** is the DTE knob for this.
- **V.42 Appendix VI.2 (informative, p.67).**
  - Many answerers run the detection phase whatever prot0 says, to catch other protocols.
  - Its NOTE says 9.3.1/V.92 requires **both** the originator and the answerer to skip it when both indicated V.42 in prot0 or in the V.92 short Phase 1 signals. The NOTE's own wording mentions only prot0 and short Phase 1, not the V.92-capability condition.
  - [DERIVED] For the answerer this means: do not wait for ODP, do not send ADP, and go straight to waiting for flags and XID (V.42 7.2.1.3 already lets a detected protocol-phase start end the wait).
- **V.42 Appendix VI.1.** An "EP" ADP sent 16 times, then EC 10 or more times, says V.42 is supported **and** the XID user data subfield may carry V.44 and manufacturer fields. `crates/ec/src/detect.rs` already recognises `P`. With the V.92 bypass, no ADP is sent, so this hint is lost. V.44 is still negotiated in XID.

### 6.3 Suspend and resume (V.42 (2002) 7.10 and 7.11; Table 2/V.42)

- The control function may issue **L-SUSPEND**. The error control function is then suspended.
  - **During the detection phase**, freeze **T400**.
  - **During protocol establishment, or once connected**, freeze **T401**, and **T402 and T403** if they are implemented.
  - NOTE: this is typically used across a **retrain** or **V.92 modem-on-hold**.
- **L-RESUME** unfreezes the frozen timers.
- Nothing else changes. The link is **not** released, so sequence numbers, windows and the V.42 bis/V.44 dictionaries survive the hold [DERIVED].
- V.44 (re)initialisation, C-INIT, happens only at link set-up (V.44 7.3 and 7.4).

### 6.4 Negotiating V.44 through V.42 XID (Table 11c/V.42 p.59; Table A.1/V.44 pp.32-33)

The user data subfield has **GI = `11111111`**. Its parameters, with bit strings written bit 8 first:

| PI (decimal / binary) | PL | Parameter | PV |
|---|---|---|---|
| 64 / `01000000` | 3 | V.44 parameter set identifier | "V44": `01010110 00110100 00110100` (V.44 Table A.1) |
| 65 / `01000001` | 1 | V.44 capability C0 | `PM00000N`. PM: 00 = no packet methods (modem connections only), 01 = invalid, 10 = packet method, 11 = packet and multi-packet (P and M are ignored for modems). N: 0 = negotiate by XID with the parameters below; 1 = negotiate after link establishment. |
| 66 / `01000010` | 1 | Data compression request P0 | `000000RT`: 00 = neither, 01 = transmit only (initiator to responder), 10 = receive only, 11 = both |
| 67 / `01000011` | 2 | P1T, codewords in the transmit direction | MSB octet first |
| 68 / `01000100` | 2 | P1R, codewords in the receive direction | MSB octet first |
| 69 / `01000101` | 1 | P2T, maximum string length, transmit | |
| 70 / `01000110` | 1 | P2R, maximum string length, receive | |
| 71 / `01000111` | 2 | P3T, history length, transmit | MSB octet first |
| 72 / `01001000` | 2 | P3R, history length, receive | MSB octet first |
| 255 / `11111111` | 1 | Manufacturer-ID (V.42 NOTE 5) | The top bit of the first PV octet is 0 for IDs not assigned by ITU-T and 1 for ITU-T-assigned IDs. An unknown ID is ignored. |

- V.42 Table 11c NOTE 2 prints the set identifier's first octet as `00101010`. V.44 prints `01010110` (ASCII 'V'). **`crates/ec/src/xid.rs` uses "V44", which matches V.44.** See §8.
- This is all existing V.42/V.44 behaviour and already implemented. V.92 changes nothing here.

---

## 7. Implementation notes for BinModem

### 7.1 `crates/v8`

- **No new CM/JM fields are needed.** `Menu` already has `pcm`, `access` and `protocol`, and `joint_pcm` implements the 7.4 pairing. V.92 reuses the V.90 menu as it stands.
- **Add QC sequences to the decoder.** Add `SYNC_QC = 0x55`, and a `Heard::Qc { digital, qca, lapm, field }` variant, where `field` holds UQTS or LM.
  - Recognise a QC sequence only when `0x55` is the first octet after idle marks. Take exactly one octet after it, and confirm it with the second copy.
  - Never let a QC body octet (`0x00`, `0xE0`, …) be read as a CJ or a CM sync.
  - Today `Decoder::feed` treats `0xE0` and `0x00` as syncs wherever they appear. Once QC is supported, that needs the framer's idle-run information.
- **Add an encoder** for QC1a, QC1d, QCA1a and QCA1d (60 or 70 bits).
  - QC1x is followed immediately by CM, with no gap. CM's own leading ten ONEs are the next thing on the line.
  - The caller must be able to cut CM **mid-octet** when QCA arrives (9.2.1.1, 9.2.2.1).
- **Test comment fix.** The test `a_menu_is_read_to_its_end_and_not_to_a_length` labels tag `0b1100` as "the T.66 tag of Table 2". The rendered Table 2 gives T.66 as b0..b3 = `0 1 1 1`, which is tag value `0b1110`. The test still works, because `0b1100` is also unassigned, but the comment is wrong.

### 7.2 `crates/datapump/src/v8.rs`

- **ANSam.** The caller must see ANSam for **1 s** before sending QC1x, rather than using the Te timing alone.
- **Answerer states.** An answerer needs three new states, listed below. All V.21 channels are already there.
  1. Detect QC while ANSam plays.
  2. Send QCA.
  3. Either:
     - analogue answerer: wait for ANSpcm and send TONEq; or
     - digital answerer: send QTS, QTS\\ and ANSpcm, and detect the 980 Hz TONEq.
- **TONEq detection.** TONEq is a plain 980 Hz tone. The Tone A/B detectors in the V.34/V.90 code can be reused for it.

### 7.3 V.8 bis (only if the CRe path is wanted)

- **HDLC.** Reuse `crates/ec/src/hdlc.rs` for flags, zero-bit stuffing and the FCS. The FCS matches 7.2.7 exactly.
- **V.21.** Reuse `crates/datapump/src/v21.rs`.
- **New pieces:**
  - the dual-tone and single-tone generator and detector (Tables 1 and 2);
  - the 100 ms mark preamble;
  - an I-field builder and parser (tree coding with bit 8 and bit 7 delimiters);
  - Figures 11 and 12 as a state machine, if full transactions are wanted.

### 7.4 `crates/ec`

- **Add `suspend()` and `resume()`** to the stack, following V.42 7.10 and 7.11. They freeze T400, or T401 and T402/T403, and unfreeze them. Call them around V.92 hold, and around retrains.
  - [DERIVED] The same hook should let the stack survive the long silence of a V.34/V.90 retrain. The memory note "V.32 retrain stall is the start-up" is related.
- **Answerer-side bypass.** `Stack::without_detection()` exists for the originator. V.92 9.2.5 and 9.3.1 need an **answerer-side** equivalent: no ODP wait, no ADP, straight to waiting for flags and XID.
- **Trigger for the bypass:**
  - both P bits (short Phase 1); or
  - both prot0 = LAPM **and** both INFO0 V.92 bits (full Phase 1).
- **When the bypass does not apply**, keep `declared_lapm()`.

### 7.5 `crates/at`

- **Add the eight `+P` commands** with the defaults in 5.1:
  - `+PMHT`: pick a default and document it. Suggest **0 (deny)** until hold is implemented, and `+PMHT=?` should then list only `(0)`.
  - `+PMHR` and `+PMHF` parse as **Execute**.
  - `+PMHR` must be able to reply late (an unsolicited-style `+PMHR: n` before the final result).
  - Return ERROR from `+PMHR` when `+PMH=1` or the DCE is idle, and from `+PMHF` when not on hold.
  - Test replies should list only what is really supported. V.250 5.4.2 applies, as the existing `+MS` code already notes.
- **`+MS`:**
  - Add `V92` to `modulations` once V.92 works. `"B103"` is fine as a proprietary string, since it does not start with "V".
  - The current `+MS=?` reply prints the rate ranges `(300-4800)`, which cannot describe V.34, V.90 or V.92.
  - V.250's own example uses `(0,300-14400)`. A V.92 DCE should report something like `(0,300-56000)`, and add the `<min_rx_rate>`/`<max_rx_rate>` lists if they are implemented.
- **`+MR`:** report `+MCR: V92` and `+MRR: <tx>,<rx>`, and send them again after rate renegotiation or fast parameter exchange, since `+MR` covers renegotiated values.
- **`O` and `H`:** add the V.92 hold semantics in 5.2 (reconnect or retrain after hold; end the call without going on-hook).
- **`&F` and `Z`:** make sure the new parameters return to their defaults.

### 7.6 Hook flash on the VoIP line

`+PMHF` needs a real hook flash: about 0.5 s on-hook, then off-hook. Neither V.250 nor V.92 says how that maps onto the repo's SIP/softphone line. This is a line-layer design question (see §9).

---

## 8. Discrepancies and typos found in the Recommendations

1. **V.92 9.2.2.2** (p.49). "If QCA2a has not been detected 1 s after transmitting **QC2a**" appears in the *digital caller* clause. It should read **QC2d**.
2. **V.92 8.3.5** (p.22). "The **analogue** modem shall encode the identification field as defined in Table 14" appears for QCA2d, which the *digital* modem sends.
3. **V.250 6.8.6 `+PMHF`.** It says "go on-hook … and then return **on-hook**". It must mean off-hook; compare `!` in 6.3.1.5.
4. **V.250 6.8.4 `+PMHR`.** The heading says "Parameter", but Table I.2 says **Action**. "Read Syntax: +PMHR" has no `?`.
5. **V.250 6.8.3 `+PMHT`.** It has no "Parameter" syntax line and **no default**.
6. **V.250 6.8.8.** The heading "Text syntax" should be "Test syntax".
7. **V.42 Table 11c NOTE 2.** It gives the V.44 set identifier's first octet as `00101010`. V.44 Table A.1 gives `01010110` ('V'). Trust V.44.
8. **V.42 Appendix VI.2 NOTE.** "9.3.1/V.9.2" means 9.3.1/V.92.
9. **V.92 9.2.3 and 9.2.4 against V.8 bis 10.2.2.** V.92 asks for at least **200 ms** of silence before ANSam **or CRe**. V.8 bis asks for at least **400 ms** before CRe. 400 ms satisfies both, so use it before CRe.
10. **V.8 bis Table 1** labels the rows "Esi" and "Esr". They mean ESi and ESr.

---

## 9. Open questions

1. **QC2x information field.** Is it exactly the two I-field octets, with no S field? V.92 defines only the I field. V.8 bis 8.1 says the field is I + S (+ NS). 7.2.9 accepts 2 information octets + 2 FCS octets as a valid frame.
2. **ESr before QC2x.** V.8 bis 10.2.1 has the caller put ESr before a message response to CRe. V.92 Figures 5, 6 and 8 show QC2x alone, starting 50 ms or more into CRe. Is ESr omitted? The figures suggest so.
3. **CRe detection by segment 1 alone.** V.92 acts on "the initial 50 ms of CRe". Segment 1 (1375 + 2002 Hz) is shared by MRe, CRd and ESi, so how is it told apart? Probably by context: a caller at call set-up sees only MRe or CRe from an answerer.
4. **What counts as a "recognised connection"** for `+PSS=0` and for sending QC at all? V.92 gives no criterion. The UQTS (analogue side) and LM (digital side) must be stored per called number, or per something else.
5. **`+PMHT` default**, and whether `+PMHR` from the DTE is *required* before granting an incoming request, or whether `+PMHT` alone decides.
6. **`+PMHR: 14`.** When should it be reported? No MH code carries "never again".
7. **`+PCW=0`.** What does the DTE actually see? V.250 defines no result code for call waiting. Circuit 125 has no in-band equivalent on a serial or virtual port apart from RING. `+VCID` is V.253, which is not in `docs/specs`.
8. **Call-waiting tone detection** (CAS/SAS) is not specified by V.92 or V.250. It is network-specific. On the VoIP rig, can the far end or the softphone even deliver it?
9. **`+PMHF` hook flash on the SIP/softphone line**: a SIP INFO or RFC 4733 "flash" event, or an actual on-hook/off-hook?
10. **`+TMO V92` objects** need ITU-T V.59 (2000), which is not in `docs/specs`.
11. **`+GCAP`.** Should it list a `+P` token? V.250 Table 2 has none.
12. **`+PIG=1` on the digital-modem side.** Which INFO/CP bits carry the refusal? See the Phase 2 and Phase 4 digests.
13. **V.8 bis full transaction paths** (CRe, CRd, CL, MS and the rest). Are they worth implementing at all? No live capture so far shows a server sending CRe.
