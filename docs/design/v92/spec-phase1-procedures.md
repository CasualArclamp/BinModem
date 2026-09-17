# V.92 Phase 1 procedures: implementation digest (clauses 9.1 and 9.2)

Source: ITU-T V.92 (11/2000), `docs/specs/T-REC-V.92-200011-I.pdf`.
Clauses covered:
- 9 (intro)
- 9.1, full Phase 1
- 9.2, short Phase 1: 9.2.1 to 9.2.5 and Figures 3 to 8.

It also includes, so the file can be used on its own:
- the Phase 1 signals of 8.1 to 8.3 (QC/QCA, TONEq, ANSpcm, QTS);
- what V.90, V.8, V.8 bis, V.25, V.21, V.34 and V.42 require wherever V.92 points at them.

Written only from the Recommendations. The numbered requirements are paraphrased, not quoted.

## 0. Conventions and pages read

### 0.1 Tags

| Tag | Meaning |
|---|---|
| **[SHALL]** | mandatory in the Recommendation |
| **[MAY]** | an option the Recommendation allows |
| **[SHOULD]** | a recommendation |
| **[FIG]** | read from a rendered figure, not stated in the text |
| **[DERIVED]** | worked out from the text; not stated in it |
| **[INTERP]** | our reading of wording that is unclear. Section 10 lists them all. |

### 0.2 Units and notation
- **T** is one PCM symbol interval, 1/8000 s = 125 µs.
  - 768T = 96 ms.
  - 48T = 6 ms.
  - 3612T = 451.5 ms.
  - 301T = 37.625 ms.
- V.21 signalling is 300 bit/s, so one bit lasts 3.333 ms.
- "Detected for N s" means the signal was recognised continuously for N seconds.

### 0.3 Bit order
Clause 8 intro (V.92 p.13) applies to Tables 2-5 and 11-14:
- bit patterns are sent leftmost bit first;
- integers are sent least significant bit first.

"Bit position n" is the n-th bit in time, counting from 0.

### 0.4 Pages rendered and viewed
All at 170 dpi, with 300 dpi crops of every figure. PNGs are in the session scratchpad `v92/pages/`.

| Document | PDF pages | Content |
|---|---|---|
| V.92 | **46, 47, 48, 49, 50** | clause 9 intro, 9.1, 9.2 and all of its subclauses, Figures 3-8. Crops: `v92_pdf046_fig3_hi`, `v92_pdf047_fig4/5/6_hi`, `v92_pdf048_fig7/8_hi`. |
| V.92 | 13-23 | clause 8 intro, 8.1, 8.2.1-8.2.5, 8.3.1-8.3.6, Tables 2-14. Every entry of Tables 7-10 was checked against the generator in 3.8. |
| V.90 | 10-12 | 3.6 Ucode; Table 1, the universal PCM codewords |
| V.90 | 20 | 8.1 and 8.2.3.1: Phase 1 power; INFO carriers |
| V.90 | 36-37 | 9.1, Phase 1 |
| V.8 (11/2000) | 9-17 | clause 5 coding format; Tables 1-7; 7.1-7.4; 8.1-8.2.3; Figure 1 |
| V.8 bis (11/2000) | 10, 13, 14, 16, 17, 18, 20, 21, 25 | 3 definitions; 7.1-7.2.9; 8.1-8.3.3; Tables 1-4 and 6-3d |
| V.34 (02/98) | 46 | 11.1, Phase 1; Figure 15 |

Clause 9.2 ends on p.50 with 9.2.5. The text taken from V.25, V.21, V.42 and V.8 bis 9.8 to 10.2 is plain prose, with no tables or figures involved. It was read from the extracted text and checked for consistency.

---

## 1. Overview

V.92 clause 9 keeps V.90's four start-up phases:
1. network interaction;
2. probing and ranging;
3. equaliser and echo-canceller training and digital impairment learning;
4. final training.

**Phase 1 and Phase 2 each have a full and a short form.**

- **Full Phase 1 (9.1)** is **V.90 Phase 1**: a V.8 CM/JM exchange, optionally preceded by V.8 bis. See section 2.
- **Short Phase 1 (9.2)** replaces the CM/JM exchange with a quick exchange of new signals:
  1. **QC** (quick connect), sent by the calling modem;
  2. **QCA** (quick connect acknowledge), sent by the answering modem;
  3. then **QTS/QTS\\** and **ANSpcm**, sent by the digital modem;
  4. then **TONEq**, sent by the analogue modem.

  The exchange can follow either of two openings:
  - **V.8-style** (the answerer sends ANSam): QC1x and QCA1x, V.8-type framing;
  - **V.8 bis-style** (the answerer sends CRe): QC2x and QCA2x, V.8 bis message framing.

  The suffix `a` or `d` names the modem that sends the signal (analogue or digital), not the modem that calls.

Short Phase 1 ends in one of two places:
- **Phase 2 of V.92**, when the two modems form an analogue/digital pair (see 9.3/9.4 and `spec-phase2-procedures.md`);
- **Phase 2 of V.34**, when both modems are analogue (Figures 7 and 8).

Every short-Phase-1 branch has a way back to full V.8 or V.8 bis if the far end does not answer.

What short Phase 1 leaves out of the V.8 exchange:
- no call-function octet;
- no modulation-mode octets;
- no PCM availability or PSTN access categories;
- no CJ.

LAPM capability travels in the **P** bit of QC/QCA instead (9.2.5). The analogue/digital role travels in one bit of QC/QCA.

---

## 2. Clause 9.1: full Phase 1

**V.92 9.1 [SHALL]:** full Phase 1 follows exactly the V.90 Phase 1 procedure.

V.92 9.1 also has a NOTE (typeset oddly on p.46). Its meaning is that V.8 has no way to say "V.92" alone, so the V.92/V.90 decision has to wait until later.
- **[DERIVED]** The decision is taken in Phase 2 from two INFO0 bits (V.92 9.3):
  - INFO0d bit 27 = V.92 capable;
  - INFO0a bit 26 = V.92 capable.
- V.8 (11/2000) Table 5 labels pcm0 b5 and b6 as "V.90 **or** V.92" analogue and digital modem.

**V.92 8.1:** every full Phase 1 signal is defined in V.25, V.8 or V.8 bis.

### 2.1 Power (V.90 8.1, rendered V.90 p.20)
- **FP1-P1 [SHALL]:** Phase 1 uses V.8, optionally with V.8 bis.
- **FP1-P2 [SHALL]:** every Phase 1 signal is sent at the **nominal transmit power level**.

### 2.2 Use of V.8 bits (V.90 9.1.1, rendered V.90 pp.36-37)
- **FP1-1 [SHALL]:** setting bit **b5 of modn0** indicates V.90 capability.
  - It also implies that at least one bit is set in the V.90 availability category. In V.8 (11/2000) that category is the **PCM modem availability** octet `pcm0`.
- **FP1-2 [SHALL]:** a modem that indicates V.90 capability also indicates its **PSTN access type** with a bit of the PSTN access category (`access0`).
- **FP1-3 [SHALL]:** V.90 operation needs two V.90-capable modems, at least one of them on digital PSTN access. If both are on analogue access, both modems proceed with plain V.8 as if V.90 had not been indicated.
- **FP1-4 [SHALL]:** if the availability bits do not show an analogue/digital pair, both modems proceed with plain V.8 as if V.90 had not been indicated.
- **FP1-5 [SHALL]:** if both modems are on digital access and both can act as analogue or digital, the **call modem becomes the analogue modem** and the answer modem becomes the digital modem.

Role decision table **[DERIVED]** from FP1-3 to FP1-5 and V.8 Tables 5 and 7:

| Caller pcm0 b5 (A) / b6 (D) | Answerer pcm0 b5 / b6 | Access b7 (digital) | Result |
|---|---|---|---|
| A | D | answerer = 1 | caller analogue, answerer digital |
| D | A | caller = 1 | caller digital, answerer analogue |
| A and D | A and D | both = 1 | caller analogue, answerer digital (FP1-5) |
| none of the above | | | V.8 as if V.90 not indicated, which normally means V.34 |

### 2.3 Call modem (V.90 9.1.2)
- **FP1-C1 [SHALL]:** at the start:
  - listen for **ANS or ANSam** (V.8);
  - transmit CI, CT, CNG or nothing (V.8).
- **FP1-C2 [SHALL]:** when ANSam is detected, stay silent for **Te** (V.8, section 2.5).
- **FP1-C3 [SHALL]:** then listen for JM and send **CM** with the bits that ask for V.90 (V.92).
- **FP1-C4 [SHALL]:** once **at least two identical JM sequences** have arrived:
  1. finish the CM octet in progress;
  2. send **CJ**;
  3. stay silent for **75 ± 5 ms**;
  4. go to Phase 2.
- **FP1-C5 [SHALL]:** if **ANS** (not ANSam) is detected, continue under Annex A/V.32 bis, T.30 or another suitable Recommendation.

Figure 3/V.90 (rendered):

```
Call:    [CI,CI,...](optional) |--Te--| [CM,CM,...][CJ] 75±5 ms | Phase 2
Answer:  >=200 ms silence [ANSam      ][JM,JM,...]  75±5 ms | Phase 2
```

### 2.4 Answer modem (V.90 9.1.3)
- **FP1-A1 [SHALL]:** on connecting to the line:
  1. stay silent for **at least 200 ms**;
  2. send **ANSam** as V.8 specifies, **with phase reversals**;
  3. listen for CM and for calling-modem responses defined by other Recommendations.
- **FP1-A2 [SHALL]:** once at least 2 identical CMs showing V.90 have arrived:
  1. send **JM** and listen for CJ;
  2. when all **3 octets of CJ** have arrived, stay silent **75 ± 5 ms**;
  3. go to Phase 2.
- **FP1-A3 [SHALL]:** if a calling-modem response from another Recommendation is detected, follow that Recommendation.
- **FP1-A4 [SHALL]:** if neither CM nor a suitable response arrives within the permitted ANSam period (V.8: **5 ± 1 s**):
  1. stay silent **75 ± 5 ms**;
  2. continue under Annex A/V.32 bis, T.30 or another suitable Recommendation.

### 2.5 V.8 (11/2000) content that full Phase 1 relies on (rendered V.8 pp.9-17)

**Framing (V.8 clause 5).** CI, CM and JM each repeat the same sequence:
- **10 ONEs**;
- **10 sync bits**;
- information octets, each framed as start bit 0, bits b0..b7 (b0 first), stop bit 1.

A **category octet** has these bits in order:
- start 0;
- b0-b3 = category tag (b0 is the LSB);
- b4 = 0;
- b5-b7 = option bits;
- stop 1.

An **extension octet** has these bits in order:
- start 0;
- b0, b1, b2 = option bits;
- b3 = 0, b4 = 1, b5 = 0;
- b6, b7 = option bits;
- stop 1.

Receivers ignore reserved bits and octets.

**Table 1/V.8, sync patterns** (in time order):

| Pattern | Use |
|---|---|
| `0000000001` | CI |
| `0000001111` | CM and JM |
| **`0101010101`** | **"Defined in ITU-T V.92"**: the QC/QCA preamble |

**Octets V.90/V.92 uses** (Tables 3-7; bits listed as b0..b7):
- `callf0` data: tag 1000, b4 = 0, b5 b6 b7 = 0 1 1 ("Data (unspecified application)").
- `modn0`: tag 1010, b4 = 0.
  - b5 = 1 when the PCM availability category is present.
  - b6 = V.34 duplex.
  - b7 = V.34 half-duplex.
- `pcm0`: tag 1110, b4 = 0.
  - b5 = V.90 or V.92 **analogue** modem available.
  - b6 = V.90 or V.92 **digital** modem available.
  - b7 = V.91.
- `prot0`: tag 0101, b4 = 0, b5 b6 b7 = **1 0 0** for LAPM (V.42).
- `access0`: tag 1011, b4 = 0.
  - b5 = call DCE on a cellular connection.
  - b6 = answer DCE on a cellular connection.
  - b7 = 1 for digital network access, 0 for analogue.

**Consistency rules** (V.8 6.3, 7.3, 7.4):
- If pcm0 is present, access0 must also be present, and modn0 b5 must be 1.
- If pcm0 b5 or b6 is 1, the modulation category must be present with V.34 availability set.
- JM carries pcm0 only if CM did.
- If JM carries pcm0, operation continues under V.90, V.91 or V.92.

**Other signals:**
- **CJ** is 3 octets of all ZEROs, each with its start and stop bits, sent on V.21(L).
- **CI, CM and CJ** use V.21(L). **JM** uses V.21(H).
- **ANSam** (V.8 7.2):
  - a 2100 ± 1 Hz tone;
  - phase reversals every 450 ± 25 ms;
  - amplitude-modulated by a 15 ± 0.1 Hz sine, so the envelope runs from 0.8 ± 0.01 to 1.2 ± 0.01 of its mean;
  - power outside 2100 ± 200 Hz at least 24 dB below the power inside.

  A call DCE must not send CM unless it has detected ANSam.

**Te** (V.8 8.1.1):
- It is the silence between the end of the call signal (or, with no call signal, the detection of ANSam) and the start of CM.
- Its minimum is **0.5 s**.
- It is **≥ 1 s** if network echo cancellers are to be disabled in the V.25 manner.

**Answer DCE** (V.8 8.2):
- silent for ≥ 0.2 s after connecting;
- ANSam lasts **5 ± 1 s** unless CM or a suitable sigC ends it;
- sends JM after **2 identical CMs**;
- keeps sending JM until all 3 CJ octets have arrived;
- stops JM mid-sequence, stays silent 75 ± 5 ms, then sends sigA.

**Call DCE** (V.8 8.1.2):
- after 2 identical JMs, finishes the current octet with its start and stop bits;
- sends CJ;
- stays silent 75 ± 5 ms;
- sends sigC.

**ANS** (V.25): a 2100 ± 15 Hz tone lasting 3.3 ± 0.7 s. Phase reversals every 425-475 ms are optional (used to disable echo cancellers). It has no amplitude modulation.

### 2.6 V.34 Phase 1 (for comparison, rendered V.34 p.46)
V.34 11.1 is the same procedure with V.34 modulation bits. The receiver of JM then chooses:
- V.34 duplex → V.34 11.2;
- V.34 half-duplex → V.34 12.2;
- otherwise → plain V.8.

The analogue-analogue ending of short Phase 1 (section 5) goes to **V.34 11.2**.

### 2.7 Optional V.8 bis in full Phase 1
V.90 8.1 allows V.8 bis. The V.8 bis signals and the parts of its procedure that short Phase 1 needs are in 3.10 and 8.2.

---

## 3. Short Phase 1 signals (V.92 8.2 and 8.3, rendered pp.14-23)

V.92 8.2 and 8.3 say which opening each pair belongs to:
- **QC1a and QCA1a** (analogue modem), **QC1d and QCA1d** (digital modem): for connections that start the V.8 way (ANSam).
- **QC2a and QCA2a**, **QC2d and QCA2d**: for connections that start the V.8 bis way (CRe).

### 3.1 Modulation (8.2)
All QC/QCA information is sent at **300 bit/s FSK** on one of the two V.21 channels:

| Channel | Mean | Binary 1 (mark, F_Z) | Binary 0 (space, F_A) |
|---|---|---|---|
| **V.21(L)**, channel No. 1 | 1080 Hz | **980 Hz** | **1180 Hz** |
| **V.21(H)**, channel No. 2 | 1750 Hz | **1650 Hz** | **1850 Hz** |

Tolerances:
- V.21 allows ±6 Hz at the modulator output.
- V.8 bis 7.2 requires ±0.01% for message F_A and F_Z. It is sensible to meet ±0.01% everywhere.

| Signal | Sent by | Channel | Framing |
|---|---|---|---|
| QC1a | analogue calling modem | V.21(L) | V.8-type, Table 2 |
| QCA1a | analogue answering modem | V.21(H) | V.8-type, Table 4 |
| QC1d | digital calling modem | V.21(L) | V.8-type, Table 11 |
| QCA1d | digital answering modem | V.21(H) | V.8-type, Table 13 |
| QC2a | analogue calling modem | V.21(H) | V.8 bis message, I-field in Table 3 |
| QCA2a | analogue answering modem | V.21(L) | V.8 bis message, I-field in Table 5 |
| QC2d | digital calling modem | V.21(H) | V.8 bis message, I-field in Table 12 |
| QCA2d | digital answering modem | V.21(L) | V.8 bis message, I-field in Table 14 |

**[DERIVED]** The channels follow a pattern.
- The **type-1** signals use V.8's channels: the caller on (L), like CM; the answerer on (H), like JM.
- The **type-2** signals use V.8 bis's channels:
  - the answering station starts the transaction by sending CRe, so it is the initiating station and uses (L);
  - the calling station is the responding station and uses (H).

**Transmit level:** V.92 gives no level for QC, QCA or TONEq. **[INTERP]** Send them at the nominal transmit power, as V.90 8.1 requires for all Phase 1 signals.

### 3.2 V.8-type framed QC1a, QCA1a, QC1d, QCA1d (Tables 2, 4, 11, 13)

All four share one layout. Positions are bit numbers in time order:

| Bits | QC1a (Table 2) | QCA1a (Table 4) | QC1d (Table 11) | QCA1d (Table 13) | Meaning |
|---|---|---|---|---|---|
| 0:9 | `1111111111` | same | same | same | ten ONEs |
| 10:19 | `0101010101` | same | same | same | sync (V.8 Table 1, "defined in V.92") |
| 20 | `0` | `0` | `0` | `0` | start bit |
| 21 | `0` | `0` | **`1`** | **`1`** | 0 = analogue modem, 1 = digital modem |
| 22 | `0` | **`1`** | `0` | **`1`** | 0 = QC, 1 = QCA |
| 23 | `P` | `P` | `P` | `P` | 1 = asks for **LAPM** (V.42), see 9.2.5 |
| 24:29 | `W0XYZ1` | `W0XYZ1` | `000LM1` | `000LM1` | a: U_QTS code WXYZ (3.4); d: ANSpcm level LM (3.5); bit 29 is the stop bit |
| 30:39 | `1111111111` | same | same | same | ten ONEs |
| 40:49 | `0101010101` | same | same | same | bits 10:19 again |
| 50:59 | `000PW0XYZ1` | `001PW0XYZ1` | `010P000LM1` | `011P000LM1` | bits 20:29 again |
| 60:69 | (none) | `1111111111` | (none) | `1111111111` | ten ONEs (QCA only) |

Field positions spelled out:
- **QC1a and QCA1a:**
  - bit 24 = W, bit 25 = 0, bit 26 = X, bit 27 = Y, bit 28 = Z, bit 29 = 1.
- **QC1d and QCA1d:**
  - bits 24, 25 and 26 = 0;
  - bit 27 = L, bit 28 = M, bit 29 = 1.

Lengths:
- **QC1a and QC1d:** **60 bits (200.0 ms)**, sent **once**, then **CM follows immediately**. CM begins with its own ten ONEs and the `0000001111` sync.
- **QCA1a and QCA1d:** **70 bits (233.3 ms)**, sent **once**.

**Seen as V.8 octets [DERIVED].** Bits 20..29 form one start/stop-framed octet: b0 = bit 21, ..., b7 = bit 28, sent LSB first. Bit 25 falls on V.8's b4 and is always 0, as in a V.8 category octet, which stops a flag being simulated.
- Sync `0101010101` framed the same way is start 0, then octet **0x55**, then stop 1.
  - For comparison: CM/JM sync = octet 0xE0; CI sync = octet 0x00.
  - `crates/v8` works in these framed octets, so QC/QCA are `[0x55, info]` twice, with the ten ONEs as idle line.
- Info octet by signal:

| Signal | Info octet (LSB = b0) |
|---|---|
| QC1a | `(P<<2) \| (W<<3) \| (X<<5) \| (Y<<6) \| (Z<<7)` |
| QCA1a | `0x02 \| (P<<2) \| (W<<3) \| (X<<5) \| (Y<<6) \| (Z<<7)` |
| QC1d | `0x01 \| (P<<2) \| (L<<6) \| (M<<7)` |
| QCA1d | `0x03 \| (P<<2) \| (L<<6) \| (M<<7)` |

**Worked vectors [DERIVED].** Time order, grouped in tens.
- **QC1a**, P=1, WXYZ=`0101` (U_QTS 70):
  `1111111111 0101010101 0001001011 1111111111 0101010101 0001001011` → info octet 0xA4
- **QCA1a**, same fields:
  `1111111111 0101010101 0011001011 1111111111 0101010101 0011001011 1111111111` → 0xA6
- **QC1d**, P=1, LM=`01` (−12 dBm0):
  `1111111111 0101010101 0101000011 1111111111 0101010101 0101000011` → 0x85
- **QCA1d**, same fields:
  `1111111111 0101010101 0111000011 1111111111 0101010101 0111000011 1111111111` → 0x87
- **QC1a**, P=0, WXYZ=`1111` (cleardown from hold):
  `1111111111 0101010101 0000101111 1111111111 0101010101 0000101111` → 0xE8

### 3.3 V.8 bis-framed QC2a, QCA2a, QC2d, QCA2d (Tables 3, 5, 12, 14)

V.92 8.2.2, 8.2.4, 8.3.3 and 8.3.5 say three things:
- these signals use the **signal structure of V.8 bis clause 7**;
- they use the **information field structure of V.8 bis clause 8**;
- the identification field (I) is coded as follows:

| I-field bit | QC2a (Table 3) | QCA2a (Table 5) | QC2d (Table 12) | QCA2d (Table 14) | Meaning |
|---|---|---|---|---|---|
| 0:3 | `1011` | `1011` | `1011` | `1011` | message type |
| 4:7 | `VVVV` | `VVVV` | `VVVV` | `VVVV` | V.8 bis revision. The NOTE says it was `0100` at publication; **receivers ignore it**. |
| 8:11 | `WXYZ` | `WXYZ` | | | U_QTS code (3.4) |
| 8:9 | | | `LM` | `LM` | ANSpcm level (3.5) |
| 10:12 | | | `000` | `000` | reserved for ITU |
| 12 | `0` | `0` | | | reserved for ITU |
| 13 | `P` | `P` | `P` | `P` | 1 = asks for LAPM (9.2.5) |
| 14 | `0` | **`1`** | `0` | **`1`** | 0 = QC, 1 = QCA |
| 15 | `0` | `0` | **`1`** | **`1`** | 0 = analogue modem, 1 = digital modem |

**How this maps onto V.8 bis** (V.8 bis 7.2.2, 8.3.1, 8.3.2, Tables 3 and 4, rendered):
- **Octet order.** Octets are numbered 1..N and sent in that order. Inside an octet, **bit 1 goes first**. So V.92 I-field bit n is:
  - V.8 bis octet 1, bit n+1, for n = 0..7;
  - V.8 bis octet 2, bit n−7, for n = 8..15.
- **Message type.** The field is bits 1-4 of octet 1. V.8 bis Table 3 lists **"Defined in ITU-T V.92" as bits 4 3 2 1 = `1 1 0 1`**, which is `1011` in time order. This matches.
- **Revision.** The field is bits 5-8. V.8 bis Table 4 gives Revision 2 as bits 8 7 6 5 = `0 0 1 0`, which is `0100` in time order. This matches the NOTE.
- **Octet values** (V.8 bis: lower bit number = less significant):
  - **Octet 1 = 0x2D**. A receiver checks only `(o1 & 0x0F) == 0x0D`.
  - **Octet 2 (QC2a/QCA2a)** = `W | X<<1 | Y<<2 | Z<<3 | P<<5 | QCA<<6`, with bit 8 = 0.
  - **Octet 2 (QC2d/QCA2d)** = `L | M<<1 | P<<5 | QCA<<6 | 0x80`.
  - **[DERIVED] trap:** as a V.8 bis number the WXYZ field has **W as its least significant bit**. The Table 2 lookup reads the pattern W-X-Y-Z from left to right. Use the pattern, not a number (3.4).

**Message frame** (V.8 bis 7.2.3-7.2.9, rendered V.8 bis pp.16-17), in time order:
1. **Preamble** [SHALL]: **100 ms ± 2%** of continuous V.21 mark, which is 30 ONE bits at 300 bit/s.
2. **Opening flags** [SHALL]: **2 to 5** HDLC flags `01111110`.
3. **Information field**, whole octets. For QC2x/QCA2x this is the **2-octet I-field**; see [INTERP] I1 below.
4. **FCS**, 16 bits.
5. **Closing flags** [SHALL]: **1 to 3** flags after the FCS.

FCS and transparency rules:
- **FCS** [SHALL]: the ISO/IEC 3309 16-bit FCS.
  - Generator x^16 + x^12 + x^5 + 1.
  - Register preset to all ONEs.
  - The ones' complement of the remainder is sent.
  - It covers every bit between the last opening flag and the FCS, with stuffed zeros excluded.
  - It is sent **from the x^15 coefficient first** (V.8 bis Figure 3: bit 1 of the first FCS octet is the MSB, bit 8 of the second is the LSB).
  - The receiver's residue with no errors is `0001110100001111` (x^15..x^0).
  - **[DERIVED]** This is the usual HDLC CRC: a reflected register, polynomial 0x8408, init 0xFFFF, output = ~reg sent LSB first, good residue 0xF0B8 in the reflected register. The same FCS and bit-stuffing are already used for LAPM framing (`crates/ec/src/hdlc.rs`).
- **Transparency** [SHALL]: insert a ZERO after every five consecutive ONEs between the flags, covering the I-field and the FCS. The receiver removes them.

**Invalid frame** (V.8 bis 7.2.9), any one of:
- not properly bounded by flags;
- fewer than 3 octets between the flags;
- not a whole number of octets;
- an FCS error.

V.8 bis 9.8 says a station **inside a V.8 bis transaction** answers an invalid frame with NAK(1) and returns to its Initial state. **[INTERP]** While short Phase 1 is still waiting for QC2x/QCA2x, just ignore an invalid frame.

**[INTERP] I1.** V.92 defines only the identification field. V.8 bis 8.1 gives an information field as I, then S, then an optional NS; for ACK and NAK the S field has zero length.
- Transmit QC2x and QCA2x with an information field of **exactly the 2 I-octets**.
- Receivers should still accept and ignore any extra octets (V.8 bis 8.2.3, "ignore information that is not understood").

**Length [DERIVED]:**
- Minimum: 30 + 16 + 16 + 16 + 8 = **86 bits = 286.7 ms**, with no stuffing.
- Maximum: 30 + 40 + 32 + up to 6 stuffed bits + 24 = 132 bits, about **440 ms**.

**Worked vectors [DERIVED].** I-field and FCS in time order. Examples use the minimum of 2 opening flags and 1 closing flag.

| Signal | Fields | I-field bits | Octets | FCS reg (~CRC) | FCS bits, time order |
|---|---|---|---|---|---|
| QC2a | P=1, WXYZ=0101 | `10110100 01010100` | 2D 2A | 0x1254 | `0010101001001000` |
| QCA2a | P=1, WXYZ=0101 | `10110100 01010110` | 2D 6A | 0x5050 | `0000101000001010` |
| QC2d | P=1, LM=01 | `10110100 01000101` | 2D A2 | 0x1A14 | `0010100001011000` |
| QCA2d | P=1, LM=01 | `10110100 01000111` | 2D E2 | 0x5810 | `0000100000011010` |
| QC2a | P=0, WXYZ=1111 | `10110100 11110000` | 2D 0F | 0x64FB | `1101111100100110`, sent as `11011111` `0` `00100110`: a ZERO is stuffed after the fifth consecutive ONE, making the frame 87 bits |

Full bit stream of the first row: 30×`1`, `01111110 01111110`, `10110100 01010100`, `0010101001001000`, `01111110`. That is 86 bits.

### 3.4 U_QTS: the Ucode used for QTS (Table 2; also cited by Tables 3, 4, 5)
Codewords are from V.90 Table 1 (rendered V.90 pp.11-12). The "linear" values use V.90's 16-bit scale. Positive codewords are listed. **Negative = the same codeword with the MSB (polarity bit) cleared**, per V.90 3.6.

| WXYZ (sent W first) | U_QTS | µ-law + | µ linear | A-law + | A linear |
|---|---|---|---|---|---|
| 0000 | 61 | C2 | 1756 | E8 | 1888 |
| 0001 | 62 | C1 | 1820 | EB | 1952 |
| 0010 | 63 | C0 | 1884 | EA | 2016 |
| 0011 | 66 | BD | 2236 | 97 | 2368 |
| 0100 | 67 | BC | 2364 | 96 | 2496 |
| 0101 | 70 | B9 | 2748 | 93 | 2880 |
| 0110 | 71 | B8 | 2876 | 92 | 3008 |
| 0111 | 74 | B5 | 3260 | 9F | 3392 |
| 1000 | 75 | B4 | 3388 | 9E | 3520 |
| 1001 | 78 | B1 | 3772 | 9B | 3904 |
| 1010 | 79 | B0 | 3900 | 9A | 4032 |
| 1011 | 82 | AD | 4604 | 87 | 4736 |
| 1100 | 83 | AC | 4860 | 86 | 4992 |
| 1101 | 86 | A9 | 5628 | 83 | 5760 |
| 1110 | 87 | A8 | 5884 | 82 | 6016 |
| **1111** | **"Cleardown from on-hold state"** | none | | | |

- Ucode 0: µ-law **FF** (+0) and 7F (−0); A-law **D5** (+0) and 55 (−0).
- The µ-law formula is `0xFF − U`.
- For WXYZ = 1111, see 9.10.2.1 and `spec-modem-on-hold.md` (MOH-9.10.2.1-R10): a held modem that receives QC with this code disconnects.

**[INTERP]** V.92 does not say how the analogue modem chooses U_QTS. Presumably it comes from what the modem learnt about the line on an earlier call to the same place ("recognised connections", clause 1 j). Only the analogue modem sends U_QTS, in QC1a, QC2a, QCA1a or QCA2a.

### 3.5 LM: the ANSpcm level (Tables 11-14)

| LM (sent L first) | ANSpcm level |
|---|---|
| 00 | −9.5 dBm0 |
| 01 | −12 dBm0 |
| 10 | −15 dBm0 |
| 11 | −18 dBm0 |

Only the digital modem sends LM, in QC1d, QC2d, QCA1d or QCA2d. The value is the level the digital modem **will** use for ANSpcm.

**[DERIVED]** QC1d/QC2d carry no U_QTS, so a returning digital modem cannot clear down a held modem by sending QC (see `spec-modem-on-hold.md` Q10).

### 3.6 TONEq (8.2.5)
- **980 Hz** tone, sent by the analogue modem.
- V.92 gives no tolerance or level.
- **[DERIVED] trap:** 980 Hz is the **V.21(L) mark frequency** and also segment 2 of V.8 bis **ESi**. A V.21(L) demodulator reads TONEq as continuous ONEs.

### 3.7 QTS and QTS\ (8.3.6), sent by the digital modem
- **QTS** = **128 repetitions** of the 6-symbol pattern {+V, +0, +V, −V, −0, −V} = **768T (96 ms)**.
- **QTS\\** = **8 repetitions** of {−V, −0, −V, +V, +0, +V} = **48T (6 ms)**.
- V is the PCM codeword whose Ucode is **U_QTS**; 0 is the codeword for Ucode 0. The sign is the G.711 polarity bit.
- **[SHALL]** The **first QTS symbol is sent in data frame interval 0**. From then on the digital modem **keeps data frame alignment**, with V.90's 6-symbol data frames.
- **[DERIVED]** QTS\\ therefore starts at symbol 768, and ANSpcm starts at symbol 816. Both are data-frame boundaries.

**[DERIVED] Spectrum.** The pattern satisfies x[n+3] = −x[n], so it has only odd harmonics of 8000/6 Hz: **1333.3 Hz** and **4000 Hz** (Nyquist).
- DFT magnitudes over one period: |X1| = |X5| = 2V, |X3| = 4V.
- After the codec's reconstruction filter, the analogue modem mostly receives a **1333.3 Hz tone** at amplitude about (2/3)·V.
- QTS→QTS\\ is a **180° phase reversal** of that tone, 768 symbols after QTS begins.
- A-law ±0 decode to ±8 (V.90 scale). They also follow x[n+3] = −x[n], so the spectrum is unchanged.

V.92 does not say what the analogue modem must do with QTS. Plausible uses:
- 8 kHz timing;
- level estimation;
- finding the digital modem's data-frame phase from the reversal.

### 3.8 ANSpcm (8.3.1, Tables 6-10), sent by the digital modem
- **What it is:** a repeating sequence of PCM codewords that makes a tone of about 2100 Hz.
  - Period: **301 symbols**, containing 79 cycles, so f = 79/301·8000 = **2099.67 Hz**.
  - A **phase reversal is added every 3612 symbols**, which is 12 periods or **451.5 ms**.
- **Use:** the sequence **may** be used to check that the assumed channel characteristics are correct.
- **Level [SHALL]:** one of four levels, the one signalled by LM:

| Level | scl µ-law | scl A-law | ϑ |
|---|---|---|---|
| −9.5 dBm0 | 1334 | 667 | 0.25·π/301 |
| −12 dBm0 | 1000 | 500 | 0.25·π/301 |
| −15 dBm0 | 708 | 354 | 0.25·π/301 |
| −18 dBm0 | 500 | 250 | 0.25·π/301 |

**Generator (8.3.1, rendered).** The Recommendation says the sequence "may be generated" this way, and the result **shall** equal Tables 7-10:

`x_k = floor( scl · √2 · cos(2π·k·79/301 + ϑ) + 0.5 )`, for k = 0..300

Each x_k is then quantised to a PCM codeword by G.711.

**[DERIVED] Quantisation that reproduces all 2408 table entries exactly.** Every entry of Tables 7, 8, 9 and 10 (µ-law and A-law) was checked against the rendered pages and matches. Use standard G.711 encoders on these scales:

- **µ-law**, x on the 14-bit scale (|x| ≤ 8159):
  1. `mask = 0xFF` if x ≥ 0, `0x7F` if x < 0;
  2. `m = min(|x|, 8159) + 33`;
  3. `seg = max(0, bitlen(m) − 6)`;
  4. `q = (m >> (seg+1)) & 0xF`;
  5. `code = ((seg<<4)|q) ^ mask`.
- **A-law**, x on the 13-bit scale (|x| ≤ 4095):
  1. `mask = 0xD5` if x ≥ 0, `0x55` if x < 0;
  2. `m = min(|x|, 4095)`. Use plain |x|, **not** |x|−1;
  3. if m < 32: `seg = 0`, `q = m>>1`; otherwise `seg = bitlen(m) − 5`, `q = (m>>seg) & 0xF`;
  4. `code = ((seg<<4)|q) ^ mask`.
- Spot values, k=0:
  - −9.5 dBm0: x = 1887 → µ **A1**; x = 943 → A **88**.
  - −18 dBm0: µ **B8**, A **93**.
- The µ-law scl is twice the A-law scl, because the two scales differ by 2×.
- The RMS of x equals scl within 0.01%.
- No x is 0, so the ±0 question never arises.

**Phase reversal [DERIVED/INTERP].**
- A 180° reversal negates x. With these symmetric encoders that is exactly **XOR 0x80** on each codeword, for both laws (checked for every entry).
- 3612 = 12 × 301, so the reversal always falls on a period boundary.
- Generate by alternating 3612 symbols of the table sequence with 3612 symbols of its sign-flipped form.
- **[INTERP]** The first reversal comes 3612 symbols after ANSpcm starts, with the table's own polarity first. V.92 does not state the starting polarity.

**Table 7 typo.** The rendered Table 7, entry k=82 A-law, prints as "8". The generator gives **08**, and that is correct.

**NOTE (8.3.1).** Some network equipment is known to change the channel when it hears ANSpcm. **[DERIVED]** It is a 2100 Hz tone with phase reversals, so it disables echo cancellers (G.164/G.165-type) and may switch DCME/ADPCM to a clear channel, as ANS/ANSam do. It has **no 15 Hz AM**, so an ANSam/ANS discriminator (`crates/v8/src/ansam.rs`) will call it **ANS**.

**Full sequences [DERIVED].** These are the generator's output, identical to Tables 7-10. Each line holds k = 43·r .. 43·r+42, which is one column block of the printed table.

```
Table 7 (-9.5 dBm0) mu-law
A1 58 22 C2 A3 38 25 B0 A7 2C 2A A9 AE 26 34 A4 BC 22 4B A2 FC 22 CB A2 3C 24 B4 A6 2E 28 AA AC 27 2F A5 B8 23 41 A2 D7 21 DB A2
43 23 B8 A5 30 27 AC AA 29 2D A6 B3 24 3C A2 CA 22 72 A2 4D 22 BD A4 34 26 AE A8 2A 2B A7 AF 25 37 A3 C0 22 55 A2 5D 22 C4 A3 39
24 B1 A7 2C 2A A9 AD 26 33 A4 BB 22 48 A2 EC 22 CE A2 3E 23 B5 A5 2E 28 AB AB 28 2F A5 B6 23 3F A2 D2 22 E0 A2 45 23 BA A4 31 27
AD A9 29 2D A6 B2 24 3A A3 C7 22 67 A2 4F 22 BE A3 36 25 AF A8 2B 2B A8 AF 25 36 A3 BF 22 50 A2 65 22 C6 A3 3A 24 B2 A6 2D 29 A9
AD 27 31 A4 BA 23 46 A2 E2 22 D1 A2 3F 23 B6 A5 2F 28 AB AB 28 2E A5 B5 23 3E A2 CE 22 EA A2 48 23 BB A4 32 26 AD A9 2A 2C A7 B1
24 39 A3 C5 22 5E A2 53 22 BF A3 37 25 AF A8 2B 2B A8 AE 26 35 A4 BD 22 4D A2 6F 22 C9 A2 3B 24 B3 A6 2D 29 AA AC 27 30 A5 B9 23
43 A2 DC 22 D6 A2 40 23 B7 A5 2F 27 AC AA 28 2E A6 B4 24 3D A2 CC 22 F7 A2 4A 22 BC A4 33 26 AE A9 2A 2C A7 B0 25 38 A3 C2 22 59
Table 7 (-9.5 dBm0) A-law
88 76 08 EE 89 13 0F 9B 82 06 01 83 84 0C 1F 8E 97 09 67 88 D4 08 E7 89 17 0E 9F 8C 04 03 81 86 02 1A 8F 93 0E 69 88 F1 08 F5 88
6F 09 93 8F 1B 0D 87 80 03 04 8D 9E 0E 17 89 E6 08 53 88 65 09 94 8E 1F 0C 85 83 01 06 82 9A 0F 12 8E E8 08 73 88 49 08 EC 89 10
0F 98 8D 07 00 83 84 0D 1E 8F 96 09 60 88 DE 08 FA 89 15 0E 9C 8C 05 03 81 86 02 05 8C 9D 0E 6B 89 FC 08 C2 88 6D 09 91 8F 18 0D
87 80 00 07 8D 99 0F 11 89 E3 08 45 88 79 09 95 8E 1D 0C 85 82 01 06 82 85 0C 1D 8E EA 09 7E 88 47 08 E3 89 11 0F 99 8D 07 00 80
87 0D 18 8F 91 09 62 88 C0 08 FF 89 6A 0E 9D 8C 05 02 86 81 02 05 8C 9C 0E 15 89 FB 08 D8 88 60 09 96 8F 19 0D 84 80 00 07 8D 98
0F 10 89 ED 08 4F 88 7D 08 EB 8E 12 0C 9A 82 06 01 83 85 0C 1C 8E 94 09 65 88 5D 08 E1 89 16 0E 9E 8D 04 03 80 87 0D 1B 8F 90 09
6C 88 CB 08 F0 88 69 0E 92 8F 1A 02 86 81 03 04 8C 9F 0E 14 89 E4 08 D6 88 66 09 97 8E 1E 0C 84 83 01 06 82 9B 0F 13 89 EE 08 74
Table 8 (-12 dBm0) mu-law
A9 5D 29 C9 AA 3D 2B B7 AD 32 2F AE B4 2C 3A AB C2 29 4F A9 FD 29 D0 A9 42 2B BB AC 35 2E AF B2 2D 37 AB BD 2A 48 A9 DC 29 DF A9
4A 2A BE AB 38 2D B2 AF 2E 34 AC BA 2B 41 AA CE 29 76 A9 52 29 C3 AA 3B 2C B5 AE 30 31 AD B7 2B 3D AA C7 29 5A A9 62 29 CA AA 3E
2B B8 AD 32 2F AE B4 2C 3A AB C0 2A 4E A9 EF 29 D4 A9 44 2A BB AC 35 2E B0 B1 2D 36 AC BC 2A 46 A9 D8 29 E6 A9 4B 2A BF AB 39 2D
B3 AF 2F 33 AD B9 2B 3F AA CD 29 6B A9 56 29 C5 AA 3C 2C B6 AE 30 31 AE B6 2C 3C AA C5 29 57 A9 69 29 CC AA 3F 2B B9 AD 33 2F AF
B3 2D 39 AB BF 2A 4C A9 E7 29 D8 A9 46 2A BC AC 36 2E B1 B0 2E 36 AC BC 2A 45 A9 D5 29 ED A9 4D 2A C0 AB 39 2C B4 AF 2F 33 AD B8
2B 3F AA CB 29 64 A9 59 29 C7 AA 3D 2C B7 AD 31 30 AE B5 2C 3B AA C4 29 53 A9 72 29 CE AA 41 2B BA AC 34 2E AF B2 2D 38 AB BE 2A
4A A9 E0 29 DB A9 48 2A BD AB 37 2D B1 B0 2E 35 AC BB 2A 43 A9 D1 29 F9 A9 4F 2A C2 AB 3A 2C B4 AE 2F 32 AD B8 2B 3E AA C9 29 5E
Table 8 (-12 dBm0) A-law
83 49 00 E1 80 15 06 92 84 19 1A 85 9F 07 11 81 EE 00 79 83 D4 03 FE 80 6E 01 96 87 1C 05 9A 99 04 12 86 94 01 60 80 CB 03 CC 80
66 00 95 86 13 07 99 9A 05 1F 87 91 01 69 80 F8 03 51 83 7C 00 EF 81 16 06 9C 84 1B 18 84 92 06 14 81 E3 00 74 83 40 00 E6 80 15
06 93 87 19 1A 85 9F 07 11 81 E8 00 7A 80 DD 03 F2 80 6C 01 96 86 1C 04 9B 98 04 1D 86 97 01 62 80 F6 03 C4 80 67 00 EA 86 10 07
9E 85 05 1E 87 90 01 6B 80 E5 00 59 83 70 00 ED 81 17 06 9D 84 1B 18 84 9D 06 17 81 E2 00 71 83 5B 00 E4 80 6B 01 90 87 1E 05 85
9E 07 10 81 EB 00 64 80 DA 03 F6 80 62 01 97 86 1D 04 98 9B 04 1D 86 97 01 6D 80 F3 03 DF 80 65 00 E8 81 10 07 9F 85 05 1E 87 93
06 6A 80 E7 00 46 83 77 00 E3 81 14 06 92 84 18 1B 84 9C 06 16 81 EC 00 7D 83 53 03 FB 80 69 01 91 87 1F 05 9A 99 07 13 86 95 00
66 80 C2 03 F5 80 60 01 94 86 12 04 98 9B 05 1C 87 96 01 6F 80 FF 03 D6 83 78 00 EE 81 11 07 9F 85 1A 19 84 93 06 15 80 E1 00 4F
Table 9 (-15 dBm0) mu-law
AF 63 30 CF B1 45 33 BE B5 3A 38 B7 BC 34 41 B2 CA 30 57 AF FD 2F D8 B0 4A 31 C1 B4 3C 37 B8 BA 35 3E B3 C5 31 4E B0 E2 2F E6 B0
4F 31 C6 B2 3E 35 BA B8 37 3C B4 C0 32 49 B0 D6 2F 78 AF 59 30 CB B1 42 34 BC B6 39 3A B5 BE 33 44 B1 CE 30 5F AF 68 2F D0 B1 47
32 BF B5 3B 38 B7 BC 34 40 B2 C9 30 55 AF F3 2F DB B0 4C 31 C2 B3 3D 36 B9 BA 36 3D B3 C4 31 4D B0 DE 2F EB AF 52 30 C7 B2 3F 35
BB B8 37 3B B4 BF 32 48 B0 D4 2F 6F AF 5C 30 CC B1 43 33 BD B6 39 39 B6 BD 33 43 B1 CC 30 5D AF 6D 2F D3 B0 48 32 BF B4 3B 37 B8
BB 34 3F B2 C8 30 52 AF EC 2F DD B0 4D 31 C4 B3 3D 36 B9 B9 36 3D B3 C3 31 4C B0 DB 2F F0 AF 54 30 C8 B2 3F 34 BB B7 38 3B B5 BF
32 47 B0 D1 2F 69 AF 5F 30 CD B1 44 33 BE B6 3A 39 B6 BD 33 42 B1 CB 30 5A AF 76 2F D6 B0 49 32 C0 B4 3C 37 B8 BB 35 3F B2 C6 31
50 AF E7 2F E0 B0 4E 31 C5 B3 3E 35 BA B9 36 3C B4 C2 31 4B B0 D9 2F FB AF 57 30 CA B2 41 34 BC B7 38 3A B5 BE 33 46 B1 CF 30 64
Table 9 (-15 dBm0) A-law
9A 41 1B F8 98 6D 1E 95 9C 11 13 92 97 1F 69 99 E6 1B 76 9A D5 1A F6 9B 66 19 E9 9F 17 12 93 91 1C 15 9E ED 18 7B 9B C0 1A C4 9B
79 18 E2 99 15 1C 91 93 12 17 9F E8 19 61 9B F0 1A 56 9A 77 1B E7 98 6E 1F 97 9D 10 11 9C 95 1E 6D 98 FA 1B 4D 9A 5A 1A FE 98 63
19 EA 9C 16 13 92 97 1F 68 99 E1 1B 73 9A D3 1A F5 9B 64 18 EE 9E 14 1D 90 91 1D 14 9E EC 18 65 9B CF 1A D9 9A 7C 1B E3 99 6A 1C
96 93 12 16 9F EB 19 60 9B F2 1A 5D 9A 4B 1B E4 98 6F 1E 94 9D 10 10 9D 94 1E 6F 98 E5 1B 48 9A 5F 1A FD 9B 60 19 EB 9F 16 12 93
96 1C 6B 99 E0 1B 7C 9A DE 1A C9 9B 65 18 EC 9E 14 1D 90 90 1D 14 9E EF 18 64 9B F5 1A D2 9A 72 1B E0 99 68 1F 96 92 13 16 9C EA
19 63 9B FF 1A 58 9A 4C 1B E5 98 6C 1E 95 9D 11 10 9D 94 1E 6E 98 E7 1B 74 9A 51 1A F0 9B 61 19 E8 9F 17 12 93 96 1C 6A 99 E2 18
7E 9B C5 1A C2 9B 7B 18 ED 9E 15 1C 91 90 1D 17 9F EE 18 67 9B F7 1A D7 9A 71 1B E6 99 69 1F 97 92 13 11 9C 95 1E 62 98 F9 1B 46
Table 10 (-18 dBm0) mu-law
B8 69 39 D7 B9 4C 3B C6 BD 41 3F BE C3 3C 49 BA D0 39 5D B8 FE 38 DE B9 50 3A CA BC 44 3E BF C1 3D 46 BB CC 39 56 B9 E8 38 EB B9
57 39 CD BB 47 3C C1 BF 3E 43 BC C9 3A 4F B9 DC 38 7A B8 5F 39 D1 BA 4A 3B C4 BD 3F 40 BD C6 3B 4C BA D5 39 66 B8 6D 39 D8 B9 4D
3B C7 BC 41 3F BE C3 3C 48 BA CF 39 5B B8 F6 38 E0 B9 52 3A CA BB 44 3D BF C0 3D 45 BB CB 3A 54 B9 E4 38 EE B9 59 39 CE BA 47 3C
C2 BE 3E 42 BC C8 3A 4E B9 DB 38 73 B8 61 39 D3 BA 4B 3B C5 BD 3F 3F BD C5 3B 4B BA D3 39 62 B8 71 39 DA B9 4E 3A C8 BC 42 3E BE
C2 3C 48 BA CE 39 5A B9 EF 38 E3 B9 54 3A CB BB 45 3D C0 BF 3D 45 BB CB 3A 53 B9 E1 38 F5 B8 5B 39 CF BA 48 3C C2 BE 3E 42 BC C7
3B 4E B9 D9 39 6D B8 65 39 D5 BA 4C 3B C6 BD 40 3F BD C4 3B 4A BA D2 39 5F B8 78 38 DC B9 4F 3A C9 BC 43 3E BF C1 3C 47 BB CD 39
58 B9 EC 38 E7 B9 56 3A CC BB 46 3D C0 BF 3E 44 BC CA 3A 51 B9 DE 38 FC B8 5D 39 CF BA 49 3C C3 BE 3F 41 BD C6 3B 4D B9 D7 39 6A
Table 10 (-18 dBm0) A-law
93 5B 10 F1 90 64 16 E2 94 69 6A 95 EF 17 61 91 FE 10 49 93 D5 13 CE 90 7E 11 E6 97 6C 15 EA E9 14 62 96 E4 10 70 90 DA 13 D9 90
71 10 E5 96 63 17 E9 EA 15 6F 97 E1 11 79 90 CB 13 57 93 4C 10 FF 91 66 16 EC 94 6B 68 94 E2 16 64 91 F3 10 44 93 5F 10 F6 90 65
16 E3 97 69 6A 95 EF 17 61 91 F8 10 4A 93 D1 13 C2 90 7C 11 E6 96 6C 14 EB E8 14 6D 96 E7 11 72 90 C6 13 DC 90 77 10 FA 91 60 17
EE 95 15 6E 97 E0 11 7B 90 F5 10 53 93 40 10 FD 91 67 16 ED 94 6B 68 94 ED 16 67 91 FD 10 41 93 52 10 F4 90 7B 11 E0 97 6E 15 95
EE 17 60 91 FA 10 74 90 DD 13 C1 90 72 11 E7 96 6D 14 E8 EB 14 6D 96 E7 11 7D 90 C3 13 D0 93 75 10 F8 91 60 17 EF 95 15 6E 97 E3
16 7A 90 F7 10 5C 93 47 10 F3 91 64 16 E2 94 68 6B 94 EC 16 66 91 FC 10 4D 93 56 13 CB 90 79 11 E1 97 6F 15 EA E9 17 63 96 E5 10
76 90 DE 13 C5 90 70 11 E4 96 62 14 E8 EB 15 6C 97 E6 11 7F 90 CF 13 D4 93 48 10 F9 91 61 17 EF 95 6A 69 94 E3 16 65 90 F1 10 58
```

### 3.9 "Silence" from the digital modem
V.92 9.2 does not say which codeword makes silence.
- V.90 does say, in its rate renegotiation procedure (V.90 9.6.1.2.5): the digital modem makes silence by sending **codewords of Ucode 0 magnitude** and keeps data frame alignment while doing so.
- **[INTERP]** Use Ucode 0 (µ FF/7F, A D5/55) for every silent period of the digital modem in short Phase 1.
- Before QTS, frame alignment is not yet defined.

### 3.10 Signals borrowed from other Recommendations

| Signal | Definition |
|---|---|
| ANSam | V.8 7.2, see 2.5. |
| ANS | V.25, see 2.5. |
| CM, JM, CJ | V.8 5 to 7.4, see 2.5. |
| **CRe** | V.8 bis 7.1, rendered pp.13-14. **Segment 1** is the initiating dual tone **1375 + 2002 Hz** for **400 ms** nominal; for CRe it may be shortened to **285 ms**. **Segment 2** is **400 Hz** for **100 ms**. Tolerances: frequency ±250 ppm, duration ±2%. Power: **12 to 15 dB below** the nominal permitted power. |
| Other V.8 bis signals | Initiating pair 1375+2002 Hz: MRe (650 Hz), MRd (1150), CRe (400), CRd (1900), ESi (980). Responding pair 1529+2225 Hz: MRd, CRd, ESr (1650). Messages MS, CL, CLR, ACK and NAK use the frame in 3.3. |

### 3.11 Duration summary [DERIVED]

| Item | Bits or symbols | Duration |
|---|---|---|
| QC1a, QC1d | 60 bits | 200.0 ms |
| QCA1a, QCA1d | 70 bits | 233.3 ms |
| QC2x, QCA2x | 86 to about 132 bits | 286.7 to about 440 ms |
| QTS | 768T | 96 ms |
| QTS\\ | 48T | 6 ms |
| ANSpcm period | 301T | 37.625 ms |
| ANSpcm reversal interval | 3612T | 451.5 ms |
| Silence after QCA1d/QCA2d, then QTS + QTS\\ | | 75 ± 5 ms, then 102 ms, then ANSpcm |

---

## 4. Figures 3-8 (rendered pp.46-48, read at 300 dpi)

Arrows in the figures show cause and effect: the end of one arrow is the event detected, the head is the action it triggers. "a" = analogue modem, "d" = digital modem.

### Figure 3: analogue calling modem, answer with ANSam
```
Analogue: ----------|1 s|[QC1a][CM ......]-----------------|<=1 s|[TONEq]----
Digital : [ANSam ............][QCA1d]--75±5--[QTS 768T][QTS\ 48T][ANSpcm ....]----
```
1. ANSam has been on for 1 s → QC1a starts.
2. The end of QC1a → d stops ANSam and starts QCA1d.
3. The end of QCA1d → a stops CM.
4. d: silence 75 ± 5 ms, QTS, QTS\\, ANSpcm. One arrow marks the QTS→QTS\\ boundary on a's side (a notes it; a's transmission does not change).
5. The start of ANSpcm → **"≤1 s"** → TONEq starts.
6. The start of TONEq → ANSpcm ends.
7. The end of ANSpcm → TONEq ends.

### Figure 4: digital calling modem, answer with ANSam
```
Analogue: [ANSam ...........][QCA1a]------------------------|<=1 s|[TONEq]----
Digital : ---|1 s|[QC1d][CM ......]--75±5--[QTS][QTS\][ANSpcm ...........]----
```
1. ANSam has been on for 1 s → QC1d.
2. The end of QC1d → a stops ANSam and starts QCA1a.
3. The end of QCA1a → d stops CM.
4. d: 75 ± 5 ms of silence, measured from the end of CM, then QTS...
5. The start of ANSpcm → "≤1 s" → TONEq.
6. The TONEq and ANSpcm arrows are as in Figure 3.

### Figure 5: analogue calling modem, answer with CRe
```
Analogue: ---|>=50 ms|[QC2a]---------------------------------|1 s|[TONEq]----
Digital : [CRe .............][QCA2d]--75±5--[QTS][QTS\][ANSpcm ...........]----
```
1. CRe has been on for ≥50 ms → QC2a.
2. The end of QC2a → d **cuts CRe short** and starts QCA2d.
3. The end of QCA2d → a notes it.
4. d: 75 ± 5 ms, QTS, QTS\\, ANSpcm.
5. The start of ANSpcm → **"1 s"** → TONEq. There is no "≤" here, because there was no ANSam earlier.

### Figure 6: digital calling modem, answer with CRe
```
Analogue: [CRe .............][QCA2a]-------------------------|1 s|[TONEq]----
Digital : ---|>=50 ms|[QC2d]---------------75±5--[QTS][QTS\][ANSpcm ....]----
```
1. CRe ≥50 ms → QC2d.
2. The end of QC2d → a cuts CRe short and starts QCA2a.
3. The end of QCA2a → d starts the 75 ± 5 ms silence, then QTS...
4. The start of ANSpcm → **1 s** → TONEq.

### Figure 7: both analogue, answer with ANSam
```
Calling  : ---|1 s|[QC1a][CM ....]---------[TONEq]--75±5--[V.34 phase 2 ...]
Answering: [ANSam ........][QCA1a]--75±5--[ANSam]--75±5--[V.34 phase 2 ...]
```
1. ANSam 1 s → QC1a + CM.
2. The end of QC1a → QCA1a.
3. The end of QCA1a → CM stops.
4. The answering modem: 75 ± 5 ms of silence, then ANSam again.
5. The start of that ANSam → TONEq starts. **No 1 s wait is shown.**
6. The start of TONEq → the second ANSam ends.
7. The end of ANSam → TONEq ends.
8. Both sides: 75 ± 5 ms, then **V.34 Phase 2**.

### Figure 8: both analogue, answer with CRe
```
Calling  : ---|>=50 ms|[QC2a]------------------|1 s|[TONEq]--75±5--[V.34 phase 2]
Answering: [CRe .......][QCA2a]--75±5--[ANSam .........]--75±5--[V.34 phase 2]
```
1. CRe ≥50 ms → QC2a.
2. The end of QC2a → CRe is cut short and QCA2a starts.
3. The end of QCA2a → noted.
4. The answering modem: 75 ± 5 ms, then ANSam.
5. **ANSam for 1 s** → TONEq.
6. The TONEq arrows are as in Figure 7.
7. Both sides: 75 ± 5 ms, then V.34 Phase 2.

---

## 5. Clause 9.2: short Phase 1 procedures as numbered requirements

The requirement IDs are `SP1-<clause>-Rn`. Durations such as "for 1 s" mean continuous detection.

### 5.1 9.2.1: the calling modem is the analogue modem
- **SP1-9.2.1-R0 [SHALL]:** at the start, listen for **ANSam** (V.8).
  - **[MAY]** also listen for **CRe** (V.8 bis).
  - **[INTERP]** 9.2.1 does not mention CI, CT or CNG. See Q1.

**9.2.1.1: ANSam path**
- **R1 [SHALL]:** when ANSam has been detected **for 1 s**:
  1. send **QC1a** once;
  2. follow it at once with **CM**, repeated as V.8 specifies (section 2.5).

  CM carries the normal V.8/V.90 content (callf0, modn0 with b5, pcm0 with b5 = analogue, access0, and prot0 if LAPM is wanted).
- **R2 [SHALL]:** while sending CM, listen for **QCA1d**, **QCA1a** and **JM**.
- **R3 [SHALL]:** if **QCA1d** is detected:
  1. **stop CM immediately**, abandoning the octet in progress (no CJ);
  2. send silence;
  3. listen for **QTS then QTS\\, followed by ANSpcm**;
  4. go on to **9.2.1.3**.
- **R4 [SHALL]:** if **QCA1a** is detected (the far end is also analogue):
  1. **stop CM immediately**, abandoning the octet in progress;
  2. send silence **until ANSam is detected**;
  3. then send **TONEq**;
  4. go on to **9.2.1.4**.
  - [FIG] Figure 7: TONEq starts as soon as ANSam starts. No 1 s wait is required here.
- **R5 [SHALL]:** if **JM** is detected, continue with **V.8**, that is full Phase 1 (FP1-C4 onward).

**9.2.1.2: CRe path (only if the optional CRe detection is enabled)**
- **R6 [SHALL]:** when **the first 50 ms of CRe** have been detected:
  1. send **QC2a** once;
  2. then go **silent**;
  3. listen for **QCA2d, QCA2a, ANSam and ANS**.
  - [FIG] Figure 5 shows "≥50 ms", so sending QC2a later is acceptable.
- **R7 [SHALL]:** if **QCA2d** is detected, listen for **QTS, QTS\\, then ANSpcm** and go on to **9.2.1.3**.
- **R8 [SHALL]:** if **QCA2a** is detected:
  1. listen for **ANSam**;
  2. when ANSam has been detected **for 1 s**, send **TONEq**;
  3. go on to **9.2.1.4**.
- **R9 [SHALL]:** if **ANSam** is detected, go on to **9.2.1.1**: after 1 s of ANSam, send QC1a + CM.
  - **[INTERP]** This applies only when no QCA2x has been received. After QCA2a, R8 governs, because the answering modem sends ANSam as part of that path.
- **R10 [SHALL]:** if **ANS** is detected **for 3 s** after QC2a was sent, continue with **V.8**. V.8 8.1.1 sends ANS to Annex A/V.32 bis, T.30 or another suitable Recommendation.
- **R11 [SHALL]:** if **neither QCA2d nor QCA2a** has been detected **1 s after QC2a was sent**, continue with **V.8 bis** as the calling (responding) station, per V.8 bis 10.2.1 (section 8.2).

**9.2.1.3: waiting for ANSpcm (analogue/digital pair)**
- **R12 [SHALL]:** when **ANSpcm** has been detected **for 1 s**, send **TONEq** for **at least 50 ms**.
- **R13 [MAY]:** if **ANSam had already been detected for 1 s in 9.2.1.1**, TONEq may start **as soon as ANSpcm is detected**.
  - [FIG] Figure 3 marks this "≤1 s".
  - **[DERIVED]** This option is always available on the QC1a/QCA1d path. It is not available on the QC2a/QCA2d path, unless the modem went through 9.2.1.1 via R9.
- **R14 [SHALL]:** when ANSpcm is **no longer detected**:
  1. stop TONEq;
  2. send silence for **75 ± 5 ms**;
  3. go to **Phase 2** as the **analogue modem** (V.92 9.3 or 9.4).
  - **[INTERP]** If ANSpcm stops before TONEq has run for 50 ms, keep TONEq on until 50 ms have passed.

**9.2.1.4: both analogue**
- **R15 [SHALL]:** when ANSam is **no longer detected**:
  1. stop TONEq;
  2. send silence for **75 ± 5 ms**;
  3. go to **V.34 Phase 2** as the **call modem** (V.34 11.2.1.1: INFO0c, then Tone B).

### 5.2 9.2.2: the calling modem is the digital modem
- **SP1-9.2.2-R0 [SHALL]:** at the start, listen for **ANSam**. **[MAY]** also listen for **CRe**.

**9.2.2.1: ANSam path**
- **R1 [SHALL]:** when ANSam has been detected **for 1 s**:
  1. send **QC1d**, followed by **CM**;
  2. listen for **QCA1a, JM and ANSam**.

  CM's pcm0 has b6 = digital. access0 b7 = 1.
- **R2 [SHALL]:** if **QCA1a** is detected:
  1. **stop CM immediately**, without finishing the octet;
  2. send silence for **75 ± 5 ms**;
  3. send **QTS** (768T), then **QTS\\** (48T), then **ANSpcm** at the level announced in LM;
  4. go on to **9.2.2.3**.
  - The first QTS symbol is data frame interval 0 (3.7).
- **R3 [SHALL]:** continue with **V.8** in either of two cases:
  - **ANSam is still detected for 1 s after QC1d was sent**;
  - **JM** is detected.
  - **[DERIVED]** After this, a late QCA1a is ignored.

**9.2.2.2: CRe path**
- **R4 [SHALL]:** when **the first 50 ms of CRe** have been detected:
  1. send **QC2d**, then go silent;
  2. listen for **QCA2a, ANSam and ANS**.
  - [FIG] Figure 6: "≥50 ms".
- **R5 [SHALL]:** if **QCA2a** is detected:
  1. send silence for **75 ± 5 ms**;
  2. send **QTS, QTS\\, ANSpcm**;
  3. go on to **9.2.2.3**.
  - [FIG] Figure 6: the 75 ms is measured from the end of QCA2a.
- **R6 [SHALL]:** if **ANSam** is detected, go on to **9.2.2.1**.
- **R7 [SHALL]:** if **ANS** is detected **for 3 s** after QC2d was sent, continue with **V.8**.
- **R8 [SHALL]:** if QCA2a has not been detected **1 s after the QC was sent**, continue with **V.8 bis**.
  - The text says "after transmitting QC2a". A digital modem sends **QC2d**, so this is a typo; read it as QC2d.

**9.2.2.3: waiting for TONEq**
- **R9 [SHALL]:** while sending ANSpcm, listen for **TONEq**. When TONEq is detected:
  1. **[DERIVED from Figures 3-6]** stop ANSpcm;
  2. send silence (Ucode 0) for **75 ± 5 ms**;
  3. go to **Phase 2** as the **digital modem**.
- **Gap:** there is **no timeout** for the case where TONEq never comes. See Q6.

### 5.3 9.2.3: the answering modem is the analogue modem
- **SP1-9.2.3-R0 [SHALL]:** on connecting to the line:
  1. stay **silent for at least 200 ms**;
  2. then send **ANSam** (V.8 procedure) **or** **CRe** (V.8 bis procedure). Which one to send is the implementation's choice.
  - V.8 bis 10.2.2 itself asks for **≥400 ms** of silence before CRe. See Q11.

**9.2.3.1: ANSam was sent**
- **R1 [SHALL]:** while sending ANSam, listen for **QC1d, QC1a or CM**.
  - This applies **even if an earlier V.8 bis session timed out** (for example CRe → V.8 bis → timeout → ANSam).
- **R2 [SHALL]:** if **QC1d** is detected (the caller is digital):
  1. send **QCA1a**, then silence. [FIG] Figure 4: ANSam ends where QCA1a begins;
  2. listen for **QTS and QTS\\, then ANSpcm**;
  3. go on to **9.2.3.3**.
  - QCA1a carries this modem's U_QTS and P.
- **R3 [MAY]:** if **QC1a** is detected (both analogue):
  1. send **QCA1a**;
  2. send silence for **75 ± 5 ms**;
  3. send **ANSam** again;
  4. go on to **9.2.3.4**.
  - If the modem does not take this option, it ignores QC1a and handles the CM that follows under V.8.
- **R4 [SHALL]:** if **CM** is detected, follow the **normal V.8 procedure**: JM after 2 identical CMs, and so on.

**9.2.3.2: CRe was sent**
- **R5 [SHALL]:** while sending CRe, listen for **QC2d and V.8 bis signals**.
  - **[INTERP]** Listen for **QC2a** as well; R7 handles it, and 9.2.4.2 lists it.
- **R6 [SHALL]:** if **QC2d** is detected:
  1. **stop CRe**, even part-way through;
  2. send **QCA2a**, then silence;
  3. listen for **QTS, QTS\\, then ANSpcm**;
  4. go on to **9.2.3.3**.
- **R7 [MAY]:** if **QC2a** is detected (both analogue):
  1. send **QCA2a**;
  2. send silence for **75 ± 5 ms**;
  3. send **ANSam**;
  4. go on to **9.2.3.4**.
  - **[INTERP]** Stop CRe first, as in R6 and Figure 8.
- **R8 [SHALL]:** if any **other V.8 bis signal** is detected (anything but QC2d or QC2a), follow the **normal V.8 bis procedure**.
- **R9 [SHALL]:** if **no V.8 bis signal, no QC2d and no QC2a** has been detected **3 s after CRe was sent**:
  1. send **ANSam**;
  2. go on to **9.2.3.1**.
  - This matches V.8 bis 10.2.2's 3 s no-response point. V.8 bis offers "retransmit or clear down" there; V.92 requires ANSam.

**9.2.3.3: waiting for ANSpcm**
- **R10 [SHALL]:** when **ANSpcm** has been detected **for 1 s**, send **TONEq** for **at least 50 ms**.
- **R11 [MAY]:** if **ANSam was sent in 9.2.3.1**, TONEq may start **as soon as ANSpcm is detected**. [FIG] Figure 4 marks this "≤1 s".
- **R12 [SHALL]:** when ANSpcm is **no longer detected**:
  1. stop TONEq;
  2. send silence for **75 ± 5 ms**;
  3. go to **Phase 2** as the **analogue modem**.
- **R13 [SHALL]:** if ANSpcm has **not been detected within 2 s after QCA1a was sent**, send **ANSam** and continue with **V.8** (answer procedure, V.8 8.2.2).
- **R14 [SHALL]:** if ANSpcm has **not been detected within 2 s after QCA2a was sent**, send **ANSam** and go on to **9.2.3.1**.

**9.2.3.4: second ANSam (both analogue)**
- **R15 [SHALL]:** while sending ANSam, listen for **TONEq and CM**.
- **R16 [SHALL]:** if **CM** is detected, continue with **V.8**. This happens when the caller missed QCA1a or QCA2a.
- **R17 [SHALL]:** if **TONEq** is detected:
  1. **stop ANSam**;
  2. send silence for **75 ± 5 ms**;
  3. go to **Phase 2**. Per Figures 7-8 this is **V.34 Phase 2 as the answer modem** (V.34 11.2.1.2: INFO0a, then Tone A).
- **[INTERP]** No timeout is given. The V.8 ANSam limit of 5 ± 1 s applies, followed by FP1-A4.

### 5.4 9.2.4: the answering modem is the digital modem
- **SP1-9.2.4-R0 [SHALL]:** on connecting, stay **silent for at least 200 ms**, then send **ANSam** (V.8) or **CRe** (V.8 bis).

**9.2.4.1: ANSam was sent**
- **R1 [SHALL]:** listen for **QC1a, QC1d or CM**. This applies even if an earlier V.8 bis session timed out.
- **R2 [SHALL]:** if **QC1a** is detected:
  1. send **QCA1d** ([FIG] Figure 3: ANSam ends here);
  2. send silence for **75 ± 5 ms**;
  3. send **QTS, QTS\\, ANSpcm**;
  4. go on to **9.2.4.3**.
  - QCA1d carries this modem's LM and P.
  - The analogue modem's U_QTS, taken from **QC1a**, sets V for QTS.
- **R3 [MAY]:** if **QC1d** is detected (both digital), this modem **may take the analogue role** and go on to **9.2.3.1**, that is send QCA1a.
  - This requires a modem that can work as an analogue modem.
  - If it does not take the role, it ignores QC1d, and CM → V.8 decides (FP1-5 then makes the **caller** analogue). See Q7.
- **R4 [SHALL]:** if **CM** is detected, follow the **normal V.8 procedure**.

**9.2.4.2: CRe was sent**
- **R5 [SHALL]:** listen for **QC2a, QC2d and V.8 bis signals**.
- **R6 [SHALL]:** if **QC2a** is detected:
  1. **stop CRe**;
  2. send **QCA2d**;
  3. send silence for **75 ± 5 ms**;
  4. send **QTS, QTS\\, ANSpcm**;
  5. go on to **9.2.4.3**.
- **R7 [MAY]:** if **QC2d** is detected, **take the analogue role** and go on to **9.2.3.2**. **[INTERP]** That means acting as in 9.2.3.2-R6: stop CRe, send QCA2a, and so on.
- **R8 [SHALL]:** if any **other V.8 bis signal** is detected, follow the **normal V.8 bis procedure**.
- **R9 [SHALL]:** if **no V.8 bis signal, no QC2a and no QC2d** has been detected **3 s after CRe was sent**, send **ANSam** and go on to **9.2.4.1**.

**9.2.4.3: waiting for TONEq**
- **R10 [SHALL]:** while sending ANSpcm, listen for **TONEq**. When TONEq is detected:
  1. **[DERIVED]** stop ANSpcm;
  2. send silence for **75 ± 5 ms**;
  3. go to **Phase 2** as the **digital modem**.
- **R11 [SHALL]:** if **TONEq has not been detected within 2 s after QCA1d was sent**, send **ANSam** and continue with **V.8**.
- **R12 [SHALL]:** if **TONEq has not been detected within 2 s after QCA2d was sent**, send **ANSam** and go on to **9.2.4.1**.
- **[DERIVED] trap:** the analogue caller stops CM only when it hears QCA1d, so CM keeps arriving here for up to one round trip after QCA1d. In this state the modem must **not** act on CM (9.2.4.3 lists only TONEq).

### 5.5 9.2.5: ODP/ADP bypass
- **SP1-9.2.5-R1 [SHALL]:** if **both modems indicated LAPM capability**, the **V.42 ODP/ADP exchange is bypassed**.
  - **[DERIVED]** "Indicated" means **P = 1 in both the QC and the QCA** that were exchanged.
  - When short Phase 1 falls back to V.8, the indication comes from prot0 in CM/JM instead, and 9.3.1 applies. That rule also requires V.92 capability from INFO0; see `spec-phase2-procedures.md` R-9.2.5 and A13.
- **What is skipped** (V.42 7.2.1, `crates/ec/src/detect.rs`):
  - the originator's ODP: DC1 with even parity, 8-16 ONEs, DC1 with odd parity, 8-16 ONEs, repeated for T400 or until an ADP arrives;
  - the answerer's ADP: `E` then `C` (or NUL), at least 10 times.
- **What happens instead [DERIVED]:**
  - The **originator** goes straight to protocol establishment (V.42 7.2.1.2 allows this when detection is disabled). It sends at least 16 flags, then SABME with P = 1 (V.42 NOTE 2 to 8.x).
  - The **answerer** sends marks or flags until it sees flags or a LAPM frame. It must not wait T400 for an ODP.
  - V.42 7.2.1.1: the originator is the modem that took the calling role in the handshake. **[INTERP]** That is the V.92 calling modem.

### 5.6 What "Phase 2" means at each exit [DERIVED]

| Exit | Peer | Phase 2 procedure | This modem sends first |
|---|---|---|---|
| 9.2.1.3 / 9.2.3.3 | digital | V.92 9.3 (full) or 9.4 (short), analogue role | INFO0a (V.92 Table 16) with bit 28 = 0, then Tone A |
| 9.2.2.3 / 9.2.4.3 | analogue | V.92 9.3 or 9.4, digital role | INFO0d (Table 15) with bit 28 = 0, then Tone B |
| 9.2.1.4 | analogue, answering | V.34 11.2, call modem | INFO0c, then Tone B |
| 9.2.3.4 | analogue, calling | V.34 11.2, answer modem | INFO0a (V.34), then Tone A |

- In every case the modem uses the 75 ± 5 ms silence to prepare its receiver for the far end's INFO0 and tone (V.90 9.2.x.1.1, V.92 9.4.x.1.1, V.34 11.2.1.x.1).
- Choosing full or short V.92 Phase 2 depends on INFO0 bits (INFO0d bits 26/27, INFO0a bits 26/27), not on having used short Phase 1.
- The INFO carriers (V.90 8.2.3.1, rendered p.20):
  - the **digital** modem sends INFO on **1200 Hz**;
  - the **analogue** modem sends INFO on **2400 Hz** at −1 dB, with an 1800 Hz guard tone at −7 dB.

---

## 6. State machines [DERIVED]

Timers start when the named signal **finishes** being sent (see Q3). "det(X, t)" means X has been detected continuously for t.

### 6.1 Analogue calling modem
```
A_CALL_IDLE (silent; listen ANSam [+CRe])
  det(ANSam,1s)            -> send QC1a+CM ; ansam_seen=true ; -> A_CALL_QC1
  det(CRe first 50ms)      -> send QC2a ; silent ; t=0 ; -> A_CALL_QC2
  det(ANS)                 -> V.8 ANS path (FP1-C5)
A_CALL_QC1 (CM repeating; listen QCA1d, QCA1a, JM)
  QCA1d                    -> abort CM mid-octet ; silent ; -> A_WAIT_QTS
  QCA1a                    -> abort CM mid-octet ; silent ; -> A_WAIT_ANSAM2
  JM                       -> V.8 full Phase 1 (FP1-C4)
  (no timer in spec; see Q6)
A_CALL_QC2 (silent; listen QCA2d, QCA2a, ANSam, ANS)
  QCA2d                    -> A_WAIT_QTS
  QCA2a                    -> A_WAIT_ANSAM1S
  det(ANSam) (no QCA2x)    -> as A_CALL_IDLE ANSam rule (1 s then QC1a+CM)
  det(ANS,3s)              -> V.8 (ANS path)
  t >= 1 s, no QCA2x       -> V.8 bis responding station
A_WAIT_QTS (listen QTS, QTS\, ANSpcm)
  det(ANSpcm) && ansam_seen [MAY] or det(ANSpcm,1s) -> TONEq on ; -> A_TONEQ_PCM
A_TONEQ_PCM
  ANSpcm lost && TONEq>=50ms -> TONEq off ; silent 75±5 ms ; -> V.92 Phase 2 (analogue)
A_WAIT_ANSAM1S (after QCA2a)
  det(ANSam,1s)            -> TONEq on ; -> A_TONEQ_AM
A_WAIT_ANSAM2 (after QCA1a)
  det(ANSam)               -> TONEq on ; -> A_TONEQ_AM
A_TONEQ_AM
  ANSam lost               -> TONEq off ; silent 75±5 ms ; -> V.34 Phase 2 (call modem)
```

### 6.2 Digital calling modem
```
D_CALL_IDLE
  det(ANSam,1s)            -> send QC1d+CM ; t=0 ; -> D_CALL_QC1
  det(CRe first 50ms)      -> send QC2d ; silent ; t=0 ; -> D_CALL_QC2
D_CALL_QC1 (listen QCA1a, JM, ANSam)
  QCA1a                    -> abort CM ; silent 75±5 ; QTS,QTS\,ANSpcm ; -> D_WAIT_TONEQ
  JM                       -> V.8
  det(ANSam, 1 s after QC1d) -> V.8 (keep CM; ignore late QCA1a)
D_CALL_QC2 (listen QCA2a, ANSam, ANS)
  QCA2a                    -> silent 75±5 ; QTS,QTS\,ANSpcm ; -> D_WAIT_TONEQ
  det(ANSam)               -> D_CALL_IDLE ANSam rule
  det(ANS,3s)              -> V.8
  t >= 1 s, no QCA2a       -> V.8 bis
D_WAIT_TONEQ (sending ANSpcm)
  TONEq                    -> stop ANSpcm ; silent(Ucode0) 75±5 ; -> V.92 Phase 2 (digital)
  (no timer in spec; see Q6)
```

### 6.3 Analogue answering modem
```
A_ANS_START: silent >= 200 ms (>= 400 ms if CRe, V.8 bis) ; send ANSam -> A_ANS_AM  | send CRe -> A_ANS_CRE
A_ANS_AM (ANSam; listen QC1d, QC1a, CM; V.8 5±1 s limit)
  QC1d                     -> stop ANSam ; QCA1a ; silent ; t=0 ; kind=1 ; -> A_ANS_WAIT_PCM
  QC1a  [MAY]              -> stop ANSam ; QCA1a ; silent 75±5 ; ANSam ; -> A_ANS_AM2
  CM                       -> V.8 answer (JM after 2 identical CM)
A_ANS_CRE (CRe; listen QC2d, QC2a, V.8 bis)
  QC2d                     -> stop CRe ; QCA2a ; silent ; t=0 ; kind=2 ; -> A_ANS_WAIT_PCM
  QC2a  [MAY]              -> stop CRe ; QCA2a ; silent 75±5 ; ANSam ; -> A_ANS_AM2
  other V.8 bis signal     -> V.8 bis
  3 s after CRe, nothing   -> ANSam ; -> A_ANS_AM
A_ANS_WAIT_PCM (listen QTS, QTS\, ANSpcm)
  det(ANSpcm) && ansam_sent [MAY] or det(ANSpcm,1s) -> TONEq ; -> A_ANS_TONEQ
  t >= 2 s, no ANSpcm      -> ANSam ; kind==1 ? V.8 answer : A_ANS_AM
A_ANS_TONEQ
  ANSpcm lost && TONEq>=50ms -> TONEq off ; silent 75±5 ; -> V.92 Phase 2 (analogue)
A_ANS_AM2 (ANSam; listen TONEq, CM)
  TONEq                    -> stop ANSam ; silent 75±5 ; -> V.34 Phase 2 (answer modem)
  CM                       -> V.8 answer
```

### 6.4 Digital answering modem
```
D_ANS_START: silent >= 200 ms ; ANSam -> D_ANS_AM | CRe -> D_ANS_CRE
D_ANS_AM (listen QC1a, QC1d, CM)
  QC1a                     -> stop ANSam ; QCA1d ; silent 75±5 ; QTS,QTS\,ANSpcm ; t=0(end QCA1d) ; kind=1 ; -> D_ANS_WAIT_TONEQ
  QC1d  [MAY]              -> become analogue: behave as A_ANS_AM on QC1d
  CM                       -> V.8 answer
D_ANS_CRE (listen QC2a, QC2d, V.8 bis)
  QC2a                     -> stop CRe ; QCA2d ; silent 75±5 ; QTS,QTS\,ANSpcm ; t=0 ; kind=2 ; -> D_ANS_WAIT_TONEQ
  QC2d  [MAY]              -> become analogue: behave as A_ANS_CRE on QC2d
  other V.8 bis signal     -> V.8 bis
  3 s after CRe, nothing   -> ANSam ; -> D_ANS_AM
D_ANS_WAIT_TONEQ (ANSpcm; ignore CM)
  TONEq                    -> stop ANSpcm ; silent 75±5 ; -> V.92 Phase 2 (digital)
  t >= 2 s, no TONEq       -> ANSam ; kind==1 ? V.8 answer : D_ANS_AM
```

---

## 7. Timers and tolerances

| # | Value | Clause | Who | Meaning |
|---|---|---|---|---|
| T1 | **≥ 200 ms** | 9.2.3, 9.2.4; V.90 9.1.3.1; V.8 8.2 | answerer | silence after connecting, before ANSam or CRe |
| T2 | ≥ 400 ms | V.8 bis 10.2.2 | answerer | silence before CRe, "per the V.8 bis procedure" (Q11) |
| T3 | **1 s** | 9.2.1.1, 9.2.2.1 | caller | continuous ANSam detection before sending QC1x |
| T4 | **50 ms** (Figure: ≥50 ms) | 9.2.1.2, 9.2.2.2 | caller | initial CRe detection before sending QC2x |
| T5 | **75 ± 5 ms** | 9.2.1.3, 9.2.1.4, 9.2.2.1-3, 9.2.3.1-4, 9.2.4.1-3 | both | every silence in short Phase 1 |
| T6 | **768T = 96 ms** | 8.3.6 | digital | QTS |
| T7 | **48T = 6 ms** | 8.3.6 | digital | QTS\\ |
| T8 | **1 s** | 9.2.1.3, 9.2.3.3 | analogue | ANSpcm detection before TONEq; waived ([MAY]) if ANSam was seen or sent |
| T9 | **≥ 50 ms** | 9.2.1.3, 9.2.3.3 | analogue | minimum TONEq duration |
| T10 | **1 s** | 9.2.1.2 | analogue caller | after QC2a with no QCA2x → V.8 bis |
| T11 | **1 s** | 9.2.2.2 | digital caller | after QC2d with no QCA2a → V.8 bis |
| T12 | **3 s** | 9.2.1.2, 9.2.2.2 | caller | ANS detected continuously for 3 s after QC2x → V.8 |
| T13 | **1 s** | 9.2.2.1 | digital caller | ANSam still present 1 s after QC1d → V.8 |
| T14 | **1 s** | 9.2.1.2 R8 | analogue caller | ANSam detection after QCA2a, before TONEq |
| T15 | **3 s** | 9.2.3.2, 9.2.4.2 | answerer | after CRe with nothing heard → ANSam |
| T16 | **2 s** | 9.2.3.3 | analogue answerer | after QCA1a (→ ANSam + V.8) or QCA2a (→ ANSam + 9.2.3.1) with no ANSpcm |
| T17 | **2 s** | 9.2.4.3 | digital answerer | after QCA1d (→ ANSam + V.8) or QCA2d (→ ANSam + 9.2.4.1) with no TONEq |
| T18 | 5 ± 1 s | V.8 8.2.2 | answerer | ANSam duration if not ended by CM or sigC |
| T19 | ≥ 0.5 s (≥ 1 s for EC disable) | V.8 8.1.1 | caller | Te, full Phase 1 only |
| T20 | 100 ms ± 2% | V.8 bis 7.2.4 | QC2x/QCA2x | message preamble of marks |
| T21 | 2-5 flags / 1-3 flags | V.8 bis 7.2.5 | QC2x/QCA2x | opening and closing flags |
| T22 | 400 (or 285) + 100 ms, ±2%; ±250 ppm | V.8 bis 7.1.2-3 | CRe | segment durations and frequency tolerance |
| T23 | −12 to −15 dB re nominal | V.8 bis 7.1.4 | CRe | CRe transmit power |
| T24 | 300 bit/s; F_A/F_Z ±0.01% (V.8 bis), ±6 Hz (V.21) | 8.2; V.8 bis 7.2 | all QC/QCA | V.21 FSK |
| T25 | 2099.67 Hz; reversal every 3612T = 451.5 ms | 8.3.1 | ANSpcm | tone, compared with ANSam's 2100 ± 1 Hz and 450 ± 25 ms |
| T26 | −9.5, −12, −15, −18 dBm0 | Table 6 | ANSpcm | level set by LM |
| T27 | 5 s | V.8 bis 9.8 | V.8 bis | return to the Initial state after 5 s outside telephony/MS states |
| T28 | nominal | V.90 8.1 | Phase 1 | transmit power for Phase 1 signals |

---

## 8. Referenced text the implementer must follow

### 8.1 V.8 (11/2000)
Used by every "proceed according to V.8" and "normal V.8 procedures" branch, and by CM/JM/ANSam. The essentials are in 2.5.

Points specific to falling back from short Phase 1:
- **Caller.** It has already sent CM, which followed QC1x. It keeps sending CM. After **2 identical JMs** it finishes the octet, sends CJ, stays silent 75 ± 5 ms, and continues as V.90/V.92 (Phase 2) or as the modulation JM selected.
- **Answerer that sends ANSam again** (9.2.3.3 R13, 9.2.4.3 R11). It restarts the V.8 answer procedure 8.2.2: ANSam for 5 ± 1 s, waiting for CM.
  - The caller had stopped CM when QCA arrived, so it will not send CM again unless it too falls back.
  - See pitfall P5 (the caller has no rule for "ANSam came back").
- **V.8 ANSam must include phase reversals** for V.90 (FP1-A1).

### 8.2 V.8 bis (11/2000)
Used by the branches "proceed per V.8 bis", "normal V.8 bis procedures" and "V.8 bis signals".
- **10.2.1, calling station** at automatic answering:
  - Listen for MRe, MRd, CRe, CRd, ESi, and also ANS/ANSam. If ANS or ANSam arrives before any V.8 bis signal, leave V.8 bis and follow V.25 or V.8.
  - Tell the initiating signals apart by their **segment 2** tone.
  - On MRe or CRe, answer with MRd or CRd, **or** with a message (MS, CL or CLR) preceded by **ESr**.
  - Follow Figure 12/V.8 bis, the responding station's state diagram (not reproduced here).
- **10.2.2, answering station:**
  - Stay silent ≥ 400 ms, send MRe or CRe, and listen for MRd, CRd or ESr.
  - An OGM may follow.
  - If no V.8 bis signal arrives within 3 s, it **may** send MRe/CRe again or clear down. V.92 R9 replaces this with **ANSam**.
- **9.9, start-up after a V.8 bis transaction:**
  - The station that receives MS configures itself as the **answer** modem, whichever end placed the call.
  - The start-up is then V.8, a shortened V.8, or V.25.
  - **[DERIVED]** A V.8 start-up there sends ANSam, which is why 9.2.3.1 and 9.2.4.1 add "even when a previous V.8 bis session has timed out".
- **Table 6-3d, Data NPar(2), octet 4** (rendered p.25):
  - bit 1 = V.91;
  - **bit 2 = V.92 analogue modem**;
  - **bit 3 = V.92 digital modem** (a digital V.92 modem cannot work on an analogue PSTN connection);
  - bits 4-6 are reserved.

  These are the V.8 bis capability bits a V.92 modem advertises in CL/CLR/MS on the V.8 bis fallback path.
- **Messages from the responding station use V.21(H); messages from the initiating station use V.21(L)** (7.2).

### 8.3 V.25
ANS is 2100 ± 15 Hz for 3.3 ± 0.7 s, with optional reversals every 425-475 ms. It matters for R10/R7, the 3 s ANS rule.

### 8.4 V.21
Channel frequencies are in 3.1. Binary 1 is the lower frequency (F_Z).

### 8.5 V.90
- 9.1 is digested in section 2.
- 3.6 and Table 1 give Ucode→codeword (3.4).
- Data frames are 6 symbols (V.92 Figure 1 shows the digital data frame, i = 0..5, inside the analogue modem's 12-symbol frame).
- 8.2.3.1 gives the INFO carriers (5.6).
- 9.2.x.1.1: during the silence that ends Phase 1, prepare the receiver for INFO0 and the far tone.

### 8.6 V.34
- 11.1 (Phase 1) is compared in 2.6.
- 11.2.1.1.1 and 11.2.1.2.1 start Phase 2 after the 75 ± 5 ms silence.
  - Call modem: INFO0c, then Tone B.
  - Answer modem: INFO0a, then Tone A.
- The analogue-analogue ending of short Phase 1 uses these.

### 8.7 V.42 (03/2002)
The 7.2.1 detection phase is summarised in 5.5. T400 defaults to 750 ms in `crates/ec/src/detect.rs`.

### 8.8 G.711
Used for ANSpcm quantisation (3.8) and for the meaning of the PCM polarity bit.

---

## 9. What is new compared with V.90

1. **Short Phase 1 as a whole**, 9.2: QC/QCA, TONEq, QTS/QTS\\, ANSpcm, and the fallback web.
2. **New V.8 and V.8 bis codepoints that V.92 depends on:**
   - V.8 Table 1 sync `0101010101`;
   - V.8 Table 5 pcm0 now names "V.90 or V.92";
   - V.8 bis message type `1101` (bits 4..1), "Defined in ITU-T V.92";
   - V.8 bis Table 6-3d, the V.92 analogue and digital bits.
3. **The digital modem fixes data frame alignment during Phase 1**, at the first QTS symbol (8.3.6).
4. **ANSpcm** is an answer tone generated as PCM codewords, at one of four levels, with an exact table.
5. **ODP/ADP bypass** when both P bits are set (9.2.5).
6. **A modem can switch roles in Phase 1**: a digital answerer may act as the analogue modem (9.2.4.1, 9.2.4.2). In V.90 the call modem always became analogue.
7. **Analogue-to-analogue quick start into V.34 Phase 2** (Figures 7, 8).
8. **Modem-on-hold return uses short Phase 1.**
   - QC with U_QTS = 1111 means "cleardown from hold".
   - A held modem ignores all Phase 1 information from before the hold (9.10.2.1).
   - See `spec-modem-on-hold.md`.
9. **Full Phase 1 is unchanged** (9.1). The V.92/V.90 decision moves to the INFO0 bits.

---

## 10. Implementation notes, pitfalls, ambiguities and questions

### 10.1 Pitfalls
- **P1: TONEq and V.21(L).**
  - TONEq is 980 Hz, which is the V.21(L) mark frequency and also ESi's segment 2.
  - A V.21(L) receiver left running will output ONEs. That is harmless: a QC or CM needs the sync pattern.
  - Do not let a "carrier present" check on V.21(L) stand in for TONEq detection. Use a tone detector with no modulation.
- **P2: ANSpcm looks like ANS.**
  - ANSpcm has phase reversals but **no 15 Hz AM**, so an ANSam/ANS discriminator reports ANS.
  - Once QCA2d has been received, the 9.2.1.2 rule "ANS for 3 s → V.8" must be **switched off**.
  - Recognise ANSpcm by context: it follows QTS\\.
  - The analogue modem may also match the exact codeword pattern for the announced LM. The 8.3.1 "verify channel characteristics" use suggests doing so.
- **P3: ANSam versus ANSpcm when the digital answerer falls back.**
  - In 9.2.4.3-R11/R12 the digital modem stops ANSpcm and sends **ANSam**. Both are 2100 Hz.
  - An analogue modem sending TONEq that tests only "2100 Hz present" will not notice the change and never hears "ANSpcm lost".
  - A detector that does notice ANSpcm stopping would, per R14, go **to Phase 2** while the far end is in V.8. That is a mismatch.
  - The Recommendation has no rule for "ANSam came back after QCA1d". **[INTERP]** Treat detecting the 15 Hz AM (or losing the PCM pattern) during A_TONEQ_PCM or A_WAIT_QTS as a fallback:
    1. stop TONEq;
    2. continue as a V.8 caller: send CM after 1 s of ANSam; the answerer is in V.8 8.2.2.
- **P4: time taken to spot the loss of ANSpcm.**
  - The digital modem sends INFO0d **75 ± 5 ms after it stops ANSpcm**. INFO0d is DPSK on **1200 Hz**, and TONEq at 980 Hz sits inside that band.
  - INFO0d therefore reaches the analogue modem 75 ms after the end of ANSpcm reaches it, whatever the line delay.
  - The analogue modem must detect the loss of ANSpcm and turn off TONEq, **and** its near-end echo must decay, well within 75 ms. Aim for detection in ≤ 30 ms. Otherwise its own TONEq echo spoils the start of INFO0d (fill bits and frame sync).
  - The digital modem, for its part, receives the TONEq tail for about one round trip while it listens for INFO0a (2400 Hz band). That is harmless if its filtering is adequate.
- **P5: CM keeps arriving at the digital answerer.**
  - The analogue caller stops CM only once it hears QCA1d, so CM keeps reaching the digital answerer for at least one round trip after QC1a was detected.
  - In D_ANS_WAIT_TONEQ, never feed that CM into V.8's "2 identical CM → JM" logic.
  - Likewise, the analogue answerer in A_ANS_WAIT_PCM ignores the CM that follows QC1d.
- **P6: stopping CM.** On QCA, stop **mid-octet** with no CJ. This is the opposite of V.8's "finish the current octet, then CJ" (FP1-C4).
- **P7: the CRe dual tone is shared.**
  - The first 50 ms of CRe is just 1375 + 2002 Hz. MRe, MRd, CRd and **ESi** all begin the same way; an answering station doing V.8 bis 10.2.3 sends ESi.
  - V.92 nevertheless triggers QC2x on 50 ms of that pair.
  - A non-V.92 V.8 bis answerer simply ignores QC2x (it has no ESr segment 1). The caller then drops to V.8 bis after 1 s (R11) and reacts to the signal's segment 2 there.
  - Keep the segment 2 detector running while in A_CALL_QC2 / D_CALL_QC2.
- **P8: CRe is quiet**, 12-15 dB below nominal. Set the detection threshold to suit.
- **P9: CRe overlaps QC2x.**
  - The answerer receives QC2x (V.21(H), 1650/1850 Hz) **while it is still sending CRe**; the 2002 Hz tone is close to 1850 Hz.
  - Both modems need band filtering. The answerer must stop CRe as soon as it detects QC2x.
- **P10: the WXYZ and LM bit order.**
  - The patterns are sent **W first** and **L first**.
  - In the V.8 bis octet those bits sit in the least-significant positions.
  - The Table 2/Table 11 lookups read the pattern left to right.
  - Decode by pattern (3.2, 3.3). Unit tests should use the vectors in 3.2 and 3.3.
- **P11: the P bit is in different places.**
  - QC1x/QCA1x: bit 23, which is V.8 b2 (0x04).
  - QC2x/QCA2x: I-field bit 13, which is octet 2 bit 6 (0x20).
- **P12: the fixed zeros.**
  - QC1a bit 25 and QC1d bits 24-26 are 0; the V.8 b4 = 0 rule means this is always true.
  - Receivers should still accept the signal if a reserved bit is set (V.8 clause 10 compatibility).
  - **[INTERP]** Require only bits 20 = 0, 29 = 1 and the sync pattern.
- **P13: how to decide a QC/QCA was received.**
  - Each QC/QCA carries its information twice (bits 20:29 and 50:59). The figures place the response after the **whole** QC has been received.
  - **[INTERP]** Act once both copies agree. If one copy is corrupted, V.8's philosophy ("two identical") suggests ignoring the signal.
  - Our VoIP path inserts about 20 ms of jitter concealment every few seconds (≈ 6 bits at 300 bit/s), so allow a sync search to restart on the second copy. The receiver design must also resynchronise after such slips (see the jitter memory).
- **P14: round-trip delay on the test rig.** Rory's VoIP path adds about **1.5 s round trip** (750 ms each way). The 1 s and 2 s windows in 9.2 were plainly written for short PSTN delays.
  - **CRe path** (R11/R8, 1 s after QC2x): QCA2x cannot come back sooner than 750 + about 290 (QCA2x) + 750 ≈ 1.8 s after QC2a ends. **The CRe path will always fall to V.8 bis on this rig.** Keep CRe detection off there, or measure the delay first. Stretching the 1 s is non-conformant.
  - **9.2.4.3's 2 s TONEq window** (a V.92 server answering us). Timeline in ms after QCA1d ends at the server:
    1. QCA1d reaches us at 750; we stop CM;
    2. the server sends QTS at 75, QTS\\ at 171 and ANSpcm at 177;
    3. ANSpcm reaches us at 927; detection takes about 50-100 ms;
    4. **if we take the [MAY] immediate-TONEq option**, TONEq leaves at about 1000 and reaches the server at about 1750, plus its detection time: **inside 2 s, with about 150-200 ms to spare**;
    5. **if we wait the full 1 s of ANSpcm**, TONEq arrives at about 2.75 s and the server has already gone back to ANSam (see P3).
    - **So the analogue caller must use R13 (immediate TONEq)** and keep ANSpcm detection latency low.
  - **9.2.3.3's 2 s ANSpcm window** (us answering, digital caller): 750 + 177 + 750 + detection ≈ 1.7-1.8 s. That is marginal; our ANSpcm detector must fire within about 200 ms.
  - General rule (from the memory note): a spec period that one end waits and the other end must beat is a comparison. On this path it has almost no margin.
- **P15: data frame alignment.**
  - Start the digital modem's 6-symbol frame counter at the first QTS symbol and never reset it.
  - Later phases that rely on data-frame alignment (DIL, Jd, B1d, data) inherit it. See `spec-phase2-procedures.md` P12.
- **P16: after short Phase 1, fields that V.8 would normally supply are missing.** There is no call function, no modulation octets and no access octet.
  - The analogue modem must **assume data**, V.34 duplex fallback, and that the peer's role is the one the QC/QCA a/d bit shows.
  - For the analogue-analogue case, assume **V.34 duplex** (V.34 11.2), because there was no JM to choose half-duplex.
- **P17: two typos in the Recommendation.**
  - 9.2.2.2 says "after transmitting QC2a" where it means **QC2d**.
  - Table 7, k = 82 A-law, prints "8" where it means **08**.
- **P18: 9.2.3.2's listen list.** It names only "QC2d and V.8 bis signals" but then handles QC2a. Listen for **QC2a** too, as 9.2.4.2 does.
- **P19: existing code.**
  - `crates/v8` treats the sync as a framed octet (CI 0x00, CM/JM 0xE0) and counts three zero octets as CJ. Add **0x55** as the QC/QCA sync.
  - **A QC info octet can be 0x00**: QC1a with P=0 and WXYZ=0000 gives 0x00, which is also the framed form of the **CI sync**.
    - A decoder that looks for CI sync octets could mistake that info octet for a CI start. Decode QC by position: the octet right after a 0x55 sync is the info octet.
    - A lone zero between 0x55 syncs cannot complete CJ's three zeros. Keep the CJ counter out of QC decoding anyway.
  - `crates/ec/src/hdlc.rs` already provides FCS-16 and zero-bit stuffing for the QC2x frame.
  - `crates/v8/src/ansam.rs` provides the AM discriminator needed for P2 and P3.
  - The V.90 digital path (`crates/datapump/src/v90/{ucode,pcm,sequences}.rs`) supplies Ucode → codeword for QTS.

### 10.2 Ambiguities (our readings, marked [INTERP] above)
- **I1:** the QC2x information field is exactly the 2 I-octets (no S field). Receivers accept and ignore extra octets.
- **I2:** timers "after sending X" run from the **end** of X.
- **I3:** TONEq lasts **max(50 ms, until ANSpcm is lost)**.
- **I4:** ANSpcm reversal phase: the table's polarity comes first, and the first reversal is after 3612 symbols.
- **I5:** digital modem silence = Ucode 0 codewords.
- **I6:** R9 (9.2.1.2), "ANSam → 9.2.1.1", applies only when no QCA2x has been received.
- **I7:** stop ANSam or CRe when sending QCA, as the figures show. The text says so only for CRe.
- **I8:** QC/QCA transmit level = nominal (V.90 8.1). TONEq level is also nominal.
- **I9:** in the analogue-analogue path, "Phase 2 of the start-up procedure" in 9.2.3.4 means **V.34** Phase 2 (Figures 7-8).
- **I10:** the P bits alone decide the 9.2.5 bypass after short Phase 1. V.92 capability is implied, because only V.92 modems send QC/QCA.

### 10.3 Open questions
- **Q1.** May the analogue caller send **CI, CT or CNG** before ANSam in short Phase 1? V.90 9.1.2.1 allows it; 9.2.1 does not mention it. If it does, when does the 1 s ANSam count start? V.8's Te starts at the end of the call signal.
- **Q2.** Should a QC/QCA with only one good copy be accepted (P13)?
- **Q3.** Do the "1 s / 2 s / 3 s after transmitting X" timers start at the beginning or the end of X? We chose the end.
- **Q4.** Which codewords make the digital modem's silence before QTS? Ucode 0 is assumed.
- **Q5.** What is ANSpcm's starting polarity, and where is its first reversal (I4)?
- **Q6.** There is **no timeout** for:
  - the digital caller waiting for TONEq (9.2.2.3);
  - the analogue caller waiting for QCA or JM while sending CM (9.2.1.1).

  Proposed guards:
  - analogue caller: keep V.8's behaviour (CM until JM), limited by the caller's overall connect timer;
  - digital caller: 2 s after ANSpcm starts, mirroring 9.2.4.3, then hang up or fall back to CM.

  Needs a decision.
- **Q7.** In 9.2.4.1-R3, a digital answerer that can also work as analogue meets QC1d: should BinModem take the analogue role (the [MAY]), or let V.8 make the caller analogue (FP1-5)? Either way works; we need a policy.
- **Q8.** How should the analogue modem choose **U_QTS**, and the digital modem **LM**?
  - Not specified. Suggestion: remember them per dialled number, alongside the short-Phase-2 memory.
  - Default: U_QTS = `0101` (Ucode 70); LM = `00` (−9.5 dBm0).
- **Q9.** Rig policy: disable the CRe path, and always use immediate TONEq (P14)?
- **Q10.** On the analogue-analogue path, what V.34 capabilities are assumed without JM (duplex, symbol rates from INFO0)? Assumed: the modem's normal V.34 duplex capabilities.
- **Q11.** Before CRe: the 200 ms of V.92 9.2.3/9.2.4, or the 400 ms of V.8 bis 10.2.2? V.92 says CRe is sent "according to the procedure specified in V.8 bis". Use **400 ms** before CRe and 200 ms before ANSam.
- **Q12.** Should short Phase 1 be attempted at all when this modem is the **digital** side? BinModem's digital modem is used mainly in loopback and tests. The analogue calling modem (9.2.1.1 → 9.2.1.3) is the case that matters on live calls.
- **Q13.** Should the analogue caller send QC1a on every call, or only to "recognised" numbers (clause 1 j)?
  - Sending it to a V.90-only server is harmless: its sync is not CM's, so the server ignores it and V.8 carries on.
  - It does cost 200 ms of the first CM.
