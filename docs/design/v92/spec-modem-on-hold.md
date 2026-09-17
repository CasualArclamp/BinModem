# V.92 modem-on-hold, cleardown and testing facilities: implementation digest

Source: ITU-T V.92 (11/2000), `docs/specs/T-REC-V.92-200011-I.pdf`.
Clauses covered: **8.9** (modem-on-hold signals), **9.10** (modem-on-hold procedures), **9.11**
(cleardown) and **10** (testing facilities), plus every clause of V.90, V.34, V.8 and V.92 that
they point to.

This file is meant to be implemented from without reopening the PDF. Every table, bit layout,
figure and number below was read from the **rendered** PDF pages, not from the lossy text
extraction.

| Document | PDF pages rendered and viewed | Used for |
|---|---|---|
| V.92 | 44, 45, 46 (8.9, Tables 32 and 33); 65, 66, 67, 68, 69 (9.10, 9.11, 10, Table 34, Figures 20-24); 70 (back cover, no content); neighbours 43, 47, 64 | the clauses themselves |
| V.92 | 13 (clause 8 bit-order rule), 14-15 (QC1a/QC2a, U_QTS = 1111), 21 (QC1d), 33 (Table 23 CPu), 36 (Table 24 CPus), 37-38 (Table 27 SUVu), 40 (Table 30 CPd), 48 and 50 (9.2 short Phase 1) | cross-references |
| V.90 | 20 (8.2, 8.2.3.1), 21 (Table 7 INFO0d frame), 47 (9.7 cleardown, 10 testing) | cross-references |
| V.34 | 32 (10.1.2.1 Tone A, 10.1.2.2 Tone B, 10.1.2.3.1 INFO modulation), 33 (Figure 13 spectrum template, 10.1.2.3.2 and Figure 14 CRC) | cross-references |
| V.8 | 11 (Table 4 modn), 12 (Table 5 pcm0), 17-18 (8.1.2, 8.2.2, 8.2.3) | cross-references |
| V.250 (07/2003) | 91-95, 100 (6.8.1-6.8.6, Tables 31-34, Table I.2) | related AT control (V.92 does not cite it) |

Things the text extraction got wrong, caught on the rendered pages:

- **drn bit positions.** Text gave CPu drn as 26:30. The rendered page gives **21:25** (CPu and CPus) and **22:26** (CPd).
- **QC1d bits 21 and 22.** Text showed them swapped. The rendered page gives bit 21 = 1 (digital modem) and bit 22 = 0 (QC).
- **Figure labels.** Text lost the figures' arrows and timing labels.

---

## 0. Conventions used in this digest

- **[SHALL]** means a mandatory requirement, **[SHOULD]** a recommendation, **[MAY]** an option, and
  **[INFO]** descriptive text with no requirement. **[DERIVED]** marks something this digest
  infers from a figure or from several clauses together; it is not a sentence of the
  Recommendation.
- Requirement IDs such as `MOH-9.10.1-R2` are local to this digest, for traceability in code and tests.
- Bit numbering: "LSB:MSB" ranges as in the tables. **Bit 0 is transmitted first in time** (8.9.2).
- **Bit-pattern order (clause 8, rendered PDF p.13).** In Tables 2 to 5, 11 to 24, 27 and 30 to 33,
  unless stated otherwise:
  - a value given as a **bit pattern** is sent **leftmost bit first**;
  - a value given as an **integer** is sent **least-significant bit first**.

  Tables 32 and 33 (the MH tables) are inside that range. So a pattern such as `0011` in bits
  12:15 means bit 12 = 0, bit 13 = 0, bit 14 = 1, bit 15 = 1.
- "The remote's RT" means the tone the *other* modem sends as its RT (see 8.9.1).

---

## 1. Clause 8.9: modem-on-hold signals

### 1.1 Tone RT (8.9.1)

- **What RT is [INFO].** RT is not a new tone. It is **Tone A or Tone B as V.90 8.2 defines them**.
  V.90 8.2.1 and 8.2.2 in turn say "as defined in 10.1.2.1/V.34" and "10.1.2.2/V.34".
- **MOH-8.9.1-R1 [SHALL].** A modem that sends **Tone A** in its retrain procedures:
  - sends **RT as Tone A**;
  - listens for **Tone B** during modem-on-hold procedures.
- **MOH-8.9.1-R2 [SHALL].** A modem that sends **Tone B** in its retrain procedures:
  - sends **RT as Tone B**;
  - listens for **Tone A** during modem-on-hold procedures.
- **Which modem is which (V.92 9.7, same as V.90 9.5).**
  - The **digital modem** sends **Tone B** when it initiates or responds to a retrain (9.7.1.1, 9.7.1.2). So the digital modem's RT is **Tone B (1200 Hz)**, and it listens for Tone A.
  - The **analogue modem** sends **Tone A** (9.7.2.1, 9.7.2.2). So the analogue modem's RT is **Tone A (2400 Hz + 1800 Hz guard)**, and it listens for Tone B.
  - [DERIVED] On a connection that fell back to V.34-only operation between two analogue modems, the V.34 roles apply instead: the call modem sends Tone B and the answer modem sends Tone A.

**Tone A (V.34 10.1.2.1, rendered p.32):**
- a 2400 Hz tone, sent by the V.34 answer modem (in V.90/V.92, the analogue modem);
- A to A-bar and back are 180 degree phase reversals of the 2400 Hz tone;
- while A or A-bar is sent, an **1800 Hz guard tone** is sent too, with **no phase reversals**;
- level: **Tone A at 1 dB below nominal transmit power; the guard tone at the nominal transmit
  power**. That is the literal 1998 text. See the pitfall in section 9.3: INFO sequences use a guard
  7 dB down, and the existing code uses -7 dB for Tone A as well.
- NOTE [SHOULD]: the bandwidth of a phase-reversing tone should not be limited so far that
  round-trip-delay measurement suffers.

**Tone B (V.34 10.1.2.2):**
- a 1200 Hz tone, sent by the V.34 call modem (in V.90/V.92, the digital modem);
- B to B-bar and back are 180 degree phase reversals;
- no guard tone;
- same bandwidth note as Tone A.

**Level context.** V.90 8.2 says all Phase 2 signals except L1 go at the nominal transmit power
level. V.92 3.4 defines nominal transmit power as the reference power the user configures.
For the digital modem's Phase 2 nominal power, see V.90 INFO0d bits 29:32.

**Durations of RT.** 8.9.1 gives none. The minimum durations are in 9.10.1 (section 2.2 below).

### 1.2 MH sequences (8.9.2)

**Purpose [INFO].** MH sequences carry the information exchanged during modem-on-hold procedures.

**MOH-8.9.2-R1 [SHALL]: modulation.** MH sequences use **the same modulation as the Phase 2 INFO
sequences of V.90 8.2.3.1** (rendered V.90 p.20). That modulation is fully specified as follows:

- binary **DPSK at 600 bit/s ± 0.01 %**, one bit per symbol;
- a **1** bit rotates the transmit point **180 degrees** from the previous point, and a **0** bit leaves it where it was (0 degrees);
- each INFO sequence is preceded by one point at an arbitrary carrier phase;
  - when several sequences are sent as a group, only the first one gets that leading point;
  - [DERIVED] MH sequences are sent back to back (9.10.1), so a run of MH sequences is one group;
  - [DERIVED] when MH follows RT directly, the RT tone is on the same carrier frequency and has no reversals (all zeros in DPSK terms), so it already serves as the phase reference;
- **analogue modem (upstream direction):**
  - carrier **2400 Hz ± 0.01 %** at **1 dB below** nominal transmit power;
  - plus an **1800 Hz ± 0.01 % guard tone 7 dB below** nominal transmit power;
- **digital modem (downstream direction):**
  - carrier **1200 Hz ± 0.01 %** at the **nominal** transmit power;
  - no guard tone;
- the line signal's magnitude spectrum must lie inside **Figure 13/V.34**. Read from the rendered
  figure, in dB relative to the carrier and symmetric about it:
  - **upper limit:**
    - +0.75 dB for |Δf| ≤ 125 Hz;
    - falling linearly to -2 dB at 300 Hz, -5 dB at 400 Hz, -9 dB at 450 Hz and -20 dB at 550 Hz;
  - **lower limit:**
    - -0.75 dB for |Δf| ≤ 125 Hz;
    - falling to -4 dB at 300 Hz, -9 dB at 400 Hz and -20 dB at 475 Hz;
  - NOTE [SHOULD, "highly desirable"]: use linear-phase separation and shaping filters, because nothing trains an equalizer for these signals;
- no scrambler, no differential encoder beyond the DPSK itself, and no trellis coding. INFO-type
  modulation has none of these.

**MOH-8.9.2-R2 [SHALL]: CRC.** The CRC generator is the one in **10.1.2.3.2/V.34** (section 1.2.4 below).

**MOH-8.9.2-R3 [SHALL]: bit order.** Bit fields are as in Table 32, and **bit 0 is transmitted first in time**.

#### 1.2.1 Table 32/V.92: MH sequence layout (rendered p.45)

| Bits (LSB:MSB) | Width | Content | Notes |
|---|---|---|---|
| 0:3 | 4 | Fill bits `1111` | |
| 4:11 | 8 | Frame sync `01110010`, **left-most bit first in time** | bit4=0, bit5=1, bit6=1, bit7=1, bit8=0, bit9=0, bit10=1, bit11=0. The same sync as every V.34/V.90 INFO sequence (Table 7/V.90). |
| 12:15 | 4 | **Signal indication bits** | see 1.2.2 |
| 16:19 | 4 | **Information bits** | see 1.2.2 |
| 20:35 | 16 | **CRC** | over bits 12:19 only; CRC bit 0 (LSB) sent first, at bit 20 |
| 36:39 | 4 | Fill bits `1111` | |

- **Total length: 40 bits**, which is **66.67 ms** at 600 bit/s. [DERIVED]
- There are **no start bits** in an MH sequence. V.34/V.90 INFO frames have none either.
- An MH sequence has the same shape as a V.34 INFO frame (fill, sync, information, CRC, fill), with an 8-bit information field.

#### 1.2.2 Signal indication and information fields

Patterns are shown leftmost-first-in-time: the leftmost character is bit 12 (or bit 16).

| Name | Bits 12:15 | Meaning (Table 32) | Bits 16:19 |
|---|---|---|---|
| **MHreq** | `0011` | Asks the remote modem to go on hold | repeat of bits 12:15 → `0011` |
| **MHack** | `0101` | Agrees to go on hold, and gives the timeout | **T1**, coded per Table 33 |
| **MHnack** | `0111` | Refuses hold; asks for cleardown or fast reconnect | repeat → `0111` |
| **MHclrd** | `1001` | Asks for cleardown | **cleardown reason** (below) |
| **MHcda** | `1011` | Acknowledges cleardown | repeat → `1011` |
| **MHfrr** | `1101` | Asks for fast reconnect | repeat → `1101` |

MHclrd reason codes (bits 16:19):

| Bits 16:19 | Reason |
|---|---|
| `0101` | Cleardown because of an incoming call |
| `0110` | Cleardown because of an outgoing call |
| `1010` | Cleardown for some other reason |

Table 32 notes:
- **NOTE 1.** Signal-indication combinations not listed are reserved for the ITU. **[SHOULD]** An MH sequence with an undefined combination should be ignored.
- **NOTE 2.** Bits 16:19 combinations not listed *for MHclrd* are reserved for the ITU. **[SHOULD]** The receiver should not interpret them.
  - [DERIVED] The sequence is still a valid MHclrd, and the MHcda response still applies. Only the reason is unknown.

[DERIVED] All six defined indications end in bit 15 = 1, and none is `0000` or `1111`.

#### 1.2.3 Table 33/V.92: the T1 code in MHack bits 16:19 (rendered pp.45-46)

| Bits 16:19 | T1 | | Bits 16:19 | T1 |
|---|---|---|---|---|
| `0000` | Reserved for the ITU | | `1000` | 4 min |
| `0001` | 10 s | | `1001` | 6 min |
| `0010` | 20 s | | `1010` | 8 min |
| `0011` | 30 s | | `1011` | 12 min |
| `0100` | 40 s | | `1100` | 16 min |
| `0101` | 1 min | | `1101` | **no limit** |
| `0110` | 2 min | | `1110` | Reserved for the ITU |
| `0111` | 3 min | | `1111` | Reserved for the ITU |

- [DERIVED] Read MSB-left, the codes count 1 to 13. The V.250 `+PMHT` values 1-13 and `+PMHR`
  values 1-13 use exactly these numbers (section 7).
- [DERIVED] Bit 16, sent first, is the MSB of that count.
- The Recommendation does not say what a receiver should do with a reserved T1 code (see open questions).

#### 1.2.4 CRC (10.1.2.3.2/V.34 and Figure 14/V.34, rendered V.34 p.33)

- **What goes in.** Every information bit of the sequence, **excluding frame-sync bits, start bits
  and fill bits**. For MH that is **bits 12..19 in transmission order** (8 bits).
- **Polynomial.** x^16 + x^12 + x^5 + 1.
- **Procedure:**
  1. load the shift register with **all ones**;
  2. shift the sequence in;
  3. output the register contents **starting with bit 0 of Figure 14**. **CRC bit 0 is the LSB.** Nothing is inverted.
- **Figure 14 structure:**
  - cells 15 down to 0, shifting towards 0;
  - feedback = (cell 0 output) XOR (input bit);
  - the feedback enters cell 15, and also adders placed in front of cell 10 and in front of cell 3.
  - This is the LSB-first (reflected) form: mask **0x8408**, initial value 0xFFFF, no final XOR.

```
reg = 0xFFFF
for b in bits[12..=19]:            # in time order
    fb  = (reg & 1) ^ b
    reg = reg >> 1
    if fb: reg ^= 0x8408           # taps into cells 15, 10, 3
# transmit reg bit 0 first, as MH bit 20, ... reg bit 15 as MH bit 35
```

- Receiver check [DERIVED]: running the same register over bits 12..35 leaves a residue of 0.
- The repo already has exactly this generator: `crates/datapump/src/v34/info.rs::crc` (its unit test asserts `shift(0, true) == 0x8408`).

#### 1.2.5 Worked MH vectors [DERIVED]

Computed with the algorithm above. Every row self-checks to residue 0. The script is in the session
scratchpad as `v92/mh_vectors.py`. Bits are in time order, grouped as fill | sync | indication |
information | CRC | fill.

| Sequence | CRC register | Bits 0..39 |
|---|---|---|
| MHreq | 0x03E7 | `1111 01110010 0011 0011 1110011111000000 1111` |
| MHack, T1 = 10 s (`0001`) | 0x24D5 | `1111 01110010 0101 0001 1010101100100100 1111` |
| MHack, T1 = 1 min (`0101`) | 0x05D7 | `1111 01110010 0101 0101 1110101110100000 1111` |
| MHack, T1 = no limit (`1101`) | 0x1556 | `1111 01110010 0101 1101 0110101010101000 1111` |
| MHnack | 0x01F7 | `1111 01110010 0111 0111 1110111110000000 1111` |
| MHclrd, incoming call (`0101`) | 0x374C | `1111 01110010 1001 0101 0011001011101100 1111` |
| MHclrd, outgoing call (`0110`) | 0xF140 | `1111 01110010 1001 0110 0000001010001111 1111` |
| MHclrd, other (`1010`) | 0xC0C3 | `1111 01110010 1001 1010 1100001100000011 1111` |
| MHcda | 0x02EF | `1111 01110010 1011 1011 1111011101000000 1111` |
| MHfrr | 0x04DF | `1111 01110010 1101 1101 1111101100100000 1111` |

CRC register values for MHack with each T1 code 0000..1111:

| Code | CRC | Code | CRC | Code | CRC | Code | CRC |
|---|---|---|---|---|---|---|---|
| 0000 | A0DD | 0100 | 81DF | 1000 | B05C | 1100 | 915E |
| 0001 | 24D5 | 0101 | 05D7 | 1001 | 3454 | 1101 | 1556 |
| 0010 | E2D9 | 0110 | C3DB | 1010 | F258 | 1110 | D35A |
| 0011 | 66D1 | 0111 | 47D3 | 1011 | 7650 | 1111 | 5752 |

These vectors assume the clause 8 leftmost-first rule for the 4-bit patterns. If an interop
capture ever disagrees, check that rule first.

---

## 2. Clause 9.10: modem-on-hold procedures

### 2.1 General (9.10)

- **MOH-9.10-R1 [MAY].** The MH sequences of 8.9.2 may be used to start a modem-on-hold procedure
  when the network interrupts the call for call waiting and related services.
- **MOH-9.10-R2 [SHALL].** **If an MH sequence is received, the appropriate MH sequence shall be sent in response.**
  - [DERIVED] Starting modem-on-hold is optional, but **responding is mandatory for every V.92 modem**, even one that never grants hold.
  - Such a modem must at least:
    - answer MHreq with MHnack, then handle MHcda or MHfrr;
    - answer MHclrd with MHcda;
    - answer MHfrr with ANSam.

### 2.2 Transmission of MH sequences (9.10.1)

- **MOH-9.10.1-R1 [SHALL]: RT before MH.** RT before an MH sequence is optional. If RT is sent, its duration must be:
  - **at least 20 ms** if that tone was itself preceded by another MH sequence;
  - **at least 50 ms** otherwise.

  Figures 20-24 label the initiator's RT as "**>50 ms or 0**", where 0 means no RT at all.
  [DERIVED] Implement ≥ 50 ms; the text says "at least".
- **MOH-9.10.1-R2 [SHALL]: back-to-back repetition.** MH sequences are sent **repeatedly**. The
  first 4 fill bits of each sequence follow immediately after the last 4 fill bits of the one
  before, so the stream is a continuous 40-bit cycle.
  - [DERIVED] On the line: `…CRC 1111 1111 01110010 …`, with eight ones between one CRC and the next sync.
- **MOH-9.10.1-R3 [SHALL]: no truncation.** **Every sequence that is started must be completed
  before any other signal is sent.** Switching sequences, going to RT, going to silence or going to
  ANSam all happen only at a 40-bit boundary.

### 2.3 Initiating sequences (9.10.1.1)

- **MOH-9.10.1.1-R1 [MAY]: who may initiate, and when.** MHreq, MHclrd and MHfrr may be sent to start a transaction only when both of these hold:
  - **circuit 107 has been asserted**, and
  - either **Tone RT has been received** (the remote's RT), **or an MH response sequence has been detected**.

  Where circuit 107 is asserted:
  - V.92 PCM-upstream Phase 3: 9.5.1.1.9 (digital modem, after Su-to-Su-bar during Jp) and 9.5.2.1.8 (analogue modem, after detecting Jp);
  - V.90 Phase 3: the circuit-107 markers in V.90 Figures 6-9.

  [DERIVED] So modem-on-hold is never available before Phase 3 has reached that point.
- **MOH-9.10.1.1-R2 [MAY].** MHnack may be sent **to initiate a second transaction in response to MHreq**. MHnack is therefore both:
  - a response to MHreq, and
  - an initiating sequence whose own responses are MHcda or MHfrr (Table 34).
- **MOH-9.10.1.1-R3 [SHALL].** An initiating sequence is sent **until the appropriate response is detected**.
- **MOH-9.10.1.1-R4 [SHALL]: initiator timeout.** If the appropriate response has not been detected
  after **2 s plus a round-trip delay**, the modem:
  1. **completes the sequence in progress**, then
  2. **either initiates a retrain or disconnects**.
  - The start of this timer is not stated (open question Q4).
  - [DERIVED] For "round-trip delay", use the estimate from the last Phase 2:
    - RTDEd for the digital modem, V.90 9.2.1.1.4: the time from the modem's own Tone B phase reversal at its line terminals to reception of the second Tone A phase reversal, minus 40 ms;
    - RTDEa for the analogue modem, V.90 9.2.2.1.4: the time from sending its Tone A reversal to receiving the Tone B reversal, minus 40 ms.
- **MOH-9.10.1.1-R5: a hold request can look like a retrain.**
  - [INFO] The start of a modem-on-hold transaction may be impossible to tell apart from the start of a retrain.
  - **[MAY]** So when a transaction is started **by sending Tone B** (in PCM operation that means the digital modem), the responding modem may start a retrain by sending a **Tone A phase reversal**.
  - [INFO] In that case the initiating modem "will normally" ignore the phase reversal and carry on with the modem-on-hold transaction.
  - **[SHALL]** In return, the responding modem must set its receiver to detect **both a Tone B phase reversal** (the retrain continuing) **and an initiating MH sequence**.
  - [DERIVED] What this looks like with the retrain rules (V.92 9.7.2.2 and V.90 9.2.2.1.3):
    1. the analogue responder sees Tone B for more than 50 ms;
    2. it goes silent for 70 ± 5 ms, then sends Tone A;
    3. once it has detected Tone B and has sent Tone A for at least 50 ms, it sends a Tone A reversal and waits for a Tone B reversal;
    4. in a real retrain, the digital modem answers that reversal with a Tone B reversal **40 ± 1 ms** after receiving it (V.90 9.2.1.1.3), sends **10 ms** more Tone B, then goes silent;
    5. in a hold transaction, the digital initiator instead starts MHreq, a 1200 Hz DPSK stream.
  - The Recommendation does **not** describe the mirror case, where the analogue modem initiates with Tone A (open question Q8).

### 2.4 Response sequences (9.10.1.2)

- **MOH-9.10.1.2-R1 [SHALL].** When one of the initiating sequences is detected, the modem sends the response that Table 34 gives for it.
- **MOH-9.10.1.2-R2 [SHALL]: when a response stops.** The response sequence is sent **repeatedly until** one of these happens:
  - **ANSam is detected**, or
  - **silence is detected**, or
  - **the initiating sequence has not been detected for 200 ms**.

  [DERIVED] 200 ms is three MH periods.

**Table 34/V.92 (rendered p.66):**

| Initiating MH sequence | Response |
|---|---|
| MHreq | MHack **or** MHnack |
| MHnack | MHcda **or** MHfrr |
| MHclrd | MHcda |
| MHfrr | **ANSam** (a V.8 signal, not an MH sequence) |

### 2.5 Modem-on-hold request (9.10.2.1, Figures 20-22)

The **requester** sends MHreq. The **responder** decides whether to hold.

**Requester side:**
- **MOH-9.10.2.1-R1 [INFO].** MHreq asks the remote modem to enter an on-hold state.
- **MOH-9.10.2.1-R2 [MAY]: after MHack.** After receiving MHack, the requester may do one of:
  - **keep sending MHreq for at most 30 s**;
  - **send Tone RT**;
  - **send silence**.

  [DERIVED] The requester's line is typically switched to the waiting call at this point, for example by a hook flash (V.250 `+PMHF`).
- **MOH-9.10.2.1-R3 [SHALL]: after MHnack.** After receiving MHnack, the requester responds with **MHcda or MHfrr within 10 s**.

**Responder side, granting or refusing:**
- **MOH-9.10.2.1-R4 [SHALL].** On receiving MHreq, the responder sends **MHack to grant** the request or **MHnack to refuse** it.

**Responder side, grant path (the "held" modem):**
- **MOH-9.10.2.1-R5 [SHALL]: entering hold.** Once MHack is being sent, the modem **enters the on-hold state**.
- **MOH-9.10.2.1-R6 [SHALL]: leaving MHack.** When **the remote's RT has been detected for 100 ms**, or **silence has been detected for 2 s**, the modem:
  1. stops sending MHack, completing the sequence in progress (MOH-9.10.1-R3);
  2. **sends ANSam within 80 ms**. Figure 20 shows the gap as "≤80 ms".
- **MOH-9.10.2.1-R7 [SHALL]: while on hold.**
  - Keep sending **ANSam for time T1**. T1 is the value this modem put in its own MHack.
  - Keep the receiver ready to detect **Phase 1 start-up signals**.
  - [DERIVED] This overrides the normal V.8 8.2.2 limit of ANSam to 5 ± 1 s.
- **MOH-9.10.2.1-R8 [SHALL]: T1 expiry.** If no Phase 1 signal has been detected **T1 after the end of the first MHack**, the modem leaves the on-hold state and **disconnects**.
  - With T1 = "no limit" (`1101`) this timer never fires.
- **MOH-9.10.2.1-R9 [SHALL]: remote returns.** If **QC** or **CM** is received, the modem proceeds with **Phase 1 as the answer modem**, **disregarding whatever it learned from the earlier Phase 1 signals**. See section 6.3 for which Phase 1 clauses then apply.
- **MOH-9.10.2.1-R10 [SHALL]: cleardown by QC.** If **QC** arrives with **U_QTS = `1111`** ("cleardown from on-hold state"), the modem **disconnects**.
  - The code sits in QC1a bits 24:29 (`W0XYZ1`, with WXYZ = 1111) or QC2a identification bits 8:11 (section 6.4).
- **MOH-9.10.2.1-R11 [SHALL]: cleardown by CM.** If a **CM** arrives with **no PCM modem availability category** and **zeros for every modulation-category modulation mode**, the modem:
  1. sends **JM**, also with **no PCM modem availability category and all modulation modes zero**;
  2. **disconnects after receiving CJ**.
  - This matches the V.8 8.2.3 rule that an answer DCE may disconnect on CJ (section 6.5).

**Responder side, refuse path:**
- **MOH-9.10.2.1-R12 [SHALL]: cleardown chosen.** If MHnack was sent in response to MHreq and **MHcda** is then detected, the modem **disconnects**.
- **MOH-9.10.2.1-R13 [SHALL]: fast reconnect chosen.** If **MHfrr** is detected in response to MHnack, the modem:
  1. **sends silence for up to 80 ms** (Figure 22: "≤80 ms");
  2. **sends ANSam**;
  3. **proceeds with Phase 1 as the answer modem**, disregarding the earlier Phase 1 information.
- [DERIVED, not stated in text] After sending MHcda in reply to MHnack, the requester also disconnects once:
  - its MHcda stops (MOH-9.10.1.2-R2: MHnack absent for 200 ms, or silence); or
  - MHnack stops.

  Figure 21 shows both modems ending, with nothing after.

**Figure 20: request granted [DERIVED causality, from the arrows].** X is the requester (top row), Y the responder (bottom row).

1. X sends RT (> 50 ms, or none).
2. Y, which was in DATA, detects X's RT, leaves data mode and sends its own RT. Y's row is labelled DATA before RT.
3. X detects Y's RT, ends its RT and starts MHreq.
4. Y detects MHreq, ends its RT and sends MHack.
5. X detects MHack while still sending MHreq. The arrow lands mid-MHreq, so X keeps MHreq going for a while and then stops. X's line is then drawn as silence.
6. Y detects the end of MHreq (silence or RT), stops MHack at a sequence boundary, waits ≤ 80 ms, then sends ANSam.

**Figure 21: refused, then cleardown [DERIVED].**
- Y's RT starts before X's in the drawing. Neither row shows DATA.
1. X detects Y's RT and sends MHreq.
2. Y detects MHreq and sends MHnack.
3. X detects MHnack (the arrow lands mid-MHreq), finishes MHreq, then sends MHcda.
4. Y detects MHcda and stops MHnack (then disconnects, R12).
5. X detects that MHnack has ended and stops MHcda.

**Figure 22: refused, then fast reconnect [DERIVED].**
- Same as Figure 21 up to step 3, but X sends **MHfrr**.
1. Y detects MHfrr, stops MHnack, waits ≤ 80 ms, then sends ANSam.
2. X detects that MHnack has ended and stops MHfrr.
3. [DERIVED from 9.10.2.3] X then waits for 1 s of ANSam and starts Phase 1 as the call modem.

### 2.6 Cleardown request (9.10.2.2, Figure 23)

- **MOH-9.10.2.2-R1 [INFO].** MHclrd asks for a cleardown.
- **MOH-9.10.2.2-R2 [SHALL].** The MHclrd sender puts the **reason** in the information field (bits 16:19, section 1.2.2).
- **MOH-9.10.2.2-R3 [SHALL].** When **MHcda is received**, the MHclrd sender **disconnects**.
  - This still completes the MHclrd sequence in progress (MOH-9.10.1-R3). Figure 23 shows MHclrd continuing a little past the detection arrow.
- **MOH-9.10.2.2-R4 [SHALL].** On receiving MHclrd, a modem **sends MHcda**.
- **MOH-9.10.2.2-R5 [SHALL].** The MHcda sender **disconnects** as soon as any of these happens:
  - **the remote's RT is detected**, or
  - **silence is detected**, or
  - **MHclrd has not been detected for 200 ms**.

**Figure 23 [DERIVED].**
- X sends RT (> 50 ms, or none).
- Y's RT is already running and starts earlier.
1. X detects Y's RT and sends MHclrd.
2. Y detects MHclrd, ends its RT and sends MHcda.
3. X detects MHcda (mid-MHclrd), completes the sequence and stops.
4. Y detects the end of MHclrd and stops MHcda.
5. Both disconnect.

### 2.7 Fast reconnect request (9.10.2.3, Figure 24)

- **MOH-9.10.2.3-R1 [INFO].** MHfrr asks for a fast reconnect.
- **MOH-9.10.2.3-R2 [SHALL].** Once the MHfrr sender has **detected ANSam for 1 s**, it proceeds with **Phase 1** of start-up.
  - [DERIVED] It takes the call-modem role: V.92 9.2.1.1 or 9.2.2.1 both start with "if ANSam is detected for 1 s".
- **MOH-9.10.2.3-R3 [SHALL].** A modem that detects MHfrr:
  1. **sends silence for up to 80 ms**;
  2. **sends ANSam**;
  3. **proceeds with Phase 1** (as the answer modem, per 9.10.2.1).

**Figure 24 [DERIVED].**
- The figure's label reads "MHffr", a typo for MHfrr.
1. X sends RT (> 50 ms, or none).
2. X detects Y's RT and sends MHfrr.
3. Y detects MHfrr, stops RT, waits ≤ 80 ms, then sends ANSam.
4. X detects that Y's RT has ended and stops MHfrr.
- There is no MH response sequence in this flow; ANSam is the response.

### 2.8 Role state sketch [DERIVED, a summary of 2.2-2.7]

```
DATA/any state with 107 ON
 ├─ local wants hold/clear/frr ─► TX RT (≥50 ms, optional) ─► wait remote RT or MH response
 │     └─► TX initiating seq (MHreq|MHclrd|MHfrr), repeat; timer 2 s + RTD ─► retrain | disconnect
 │           MHreq:  rx MHack  ─► [MAY] MHreq ≤30 s | RT | silence ─► (line elsewhere) ─► later: ANSam 1 s ─► Phase 1 as CALL
 │                   rx MHnack ─► within 10 s TX MHcda (─► disconnect) | MHfrr (─► ANSam 1 s ─► Phase 1 as CALL)
 │           MHclrd: rx MHcda  ─► finish seq ─► disconnect
 │           MHfrr:  rx ANSam 1 s ─► Phase 1 (CALL)
 └─ remote RT seen (treat as retrain start) ─► TX own RT; listen for BOTH retrain continuation and initiating MH
       rx MHreq  ─► TX MHack(T1) ─► ON-HOLD: on remote RT 100 ms | silence 2 s ─► finish seq ─► ≤80 ms ─► ANSam (≤T1 from end of 1st MHack)
       │                                   rx QC(U_QTS=1111) ─► disconnect
       │                                   rx CM(no pcm0, all modn 0) ─► JM(same) ─► rx CJ ─► disconnect
       │                                   rx QC | CM ─► Phase 1 as ANSWER (forget old Phase 1 info)
       │                                   T1 expires ─► disconnect
       │         or TX MHnack ─► rx MHcda ─► disconnect
       │                         rx MHfrr ─► finish seq ─► silence ≤80 ms ─► ANSam ─► Phase 1 as ANSWER
       rx MHclrd ─► TX MHcda until remote RT | silence | MHclrd absent 200 ms ─► disconnect
       rx MHfrr  ─► finish seq ─► silence ≤80 ms ─► ANSam ─► Phase 1 as ANSWER
       responses stop on: ANSam | silence | initiating seq absent 200 ms (9.10.1.2)
```

---

## 3. Clause 9.11: cleardown

- **CLR-9.11-R1 [SHALL].** A connection is ended with the cleardown procedure.
- **CLR-9.11-R2 [SHALL]: how cleardown is signalled.** Cleardown is signalled by setting **drn = 0** in a rate sequence: the analogue modem's, or the digital modem's.
  - **The text says "SUVu" and "SUVd". Those sequences have no drn field** (Table 27: bits 19:25 reserved, 26 wait-for-CPu, 27:31 level, 32 silence request, 33 acknowledge; Table 31: bits 19:31 reserved, 32 silence request, 33 acknowledge).
  - The drn fields, as read from the rendered tables, are:

| Sequence | Sent by | drn bits | Range | Rate encoded | drn = 0 |
|---|---|---|---|---|---|
| CPu, long (Table 23, type bits 19:20 = 1) | analogue | **21:25** | 0..22 | (drn + 20) × 8000/6 downstream | "indicates cleardown" |
| CPt (Table 23, type bits 19:20 = 0) | analogue | 21:25 | 0..22 | (drn + 8) × 8000/6 | same field |
| CPus (Table 24, bits 19:20 = 2) | analogue | **21:25** | 0..22 | (drn + 20) × 8000/6 | "indicates cleardown" |
| CPd (Table 30) | digital | **22:26** | 0..19 | (drn + 17) × 8000/6 upstream | "**shall** indicate cleardown" |

  - drn is an integer, so it is sent **LSB first** (clause 8 rule).
  - Interpretation, which V.90 9.7 supports: "rate sequence" means CPu/CPus from the analogue modem and CPd from the digital modem. V.90 names them "CP by the analogue modem or MP by the digital modem" (V.90 9.7, rendered V.90 p.47). The SUV names in V.92 9.11 look like an editorial slip (open question Q1).
  - [DERIVED] When the connection runs V.90-style with V.34 upstream, the V.90 rate sequences carry drn and V.90 9.7 applies.
- **CLR-9.11-R3 [MAY].** Cleardown may be signalled **whenever a modem sends a rate sequence**: Phase 4 of training, rate renegotiation (9.8) or fast parameter exchange (9.9).
- **CLR-9.11-R4 [SHALL]: clearing down from data mode.** A modem in data mode first **initiates either a rate renegotiation (9.8) or a fast parameter exchange (9.9)**, so that it can send a rate sequence with drn = 0.
  - V.90 9.7 allowed only rate renegotiation. **Fast parameter exchange as a cleardown vehicle is new in V.92.**
- **[INFO, from V.90 9.7 NOTE; V.92 does not repeat it] [SHOULD].** The digital modem should ignore the transmit and receive constellation fields of a CP with drn = 0.
  - [DERIVED] Treat every modulation parameter in a drn = 0 CPu/CPd as don't-care. Don't try to build data-mode tables from it.
- What follows drn = 0 (going on-hook, circuit 109) is not specified beyond "disconnect". [DERIVED]
  - Complete the sequence in progress, stop transmitting and release the line.
  - Don't wait for acknowledge bits. The rules on acknowledging and grouping CP sequences still apply while the exchange is running.

Short recap of the referenced 9.9 fast parameter exchange (rendered pp.64-65), for the cleardown path:

**Initiating digital modem (9.9.1.1):**
1. Turn OFF 106 and listen for RM, RM′ and SUVu.
2. Send Rf for 384T, then Rf-bar for 24T, starting on a data-frame boundary.
3. Zero the scrambler, differential encoder and spectral-shaping memory, and send SUVd with bit 32 clear.
4. After RM, RM′, receive SUVu and go to 9.6.1.1.2.
- If Ru is detected instead, go to 9.8.1.2.1.

**Initiating analogue modem (9.9.2.1):**
1. Turn OFF 106 and listen for Rf, Rf-bar and SUVd.
2. Send RM for 384T, then RM′ for 24T, on a data-frame boundary.
3. Zero the scrambler and differential encoder, and send SUVu with bit 32 clear.
4. After Rf, Rf-bar, go to 9.6.2.1.2.
- If Rd is detected instead, go to 9.8.2.2.1.

**Responders (9.9.1.2, 9.9.2.2):**
1. Clamp 104 to 1.
2. Wait for the R-to-R′/R-bar transition.
3. Send their own R signal for 384T + 24T, on a frame boundary.
4. Zero state, send SUV with bit 32 clear, and go to 9.6.x.1.2, where CPu/CPd (carrying drn) are exchanged.

Details belong to the 9.9 digest.

---

## 4. Clause 10: testing facilities

- **TST-10-R1 [INFO/normative exclusion].** Testing facilities defined in other V-series modem Recommendations (e.g. V.54 loopbacks) **cannot be used** with V.92. Suitable V.92 testing facilities are **for further study**.
  - The wording is identical to V.90 clause 10.
- [DERIVED] There is nothing normative to implement.
  - Don't advertise or run V.54-style loop tests while in V.92/V.90 PCM mode.
  - Internal simulation loopbacks and vector tests remain our own business.

---

## 5. Timers and tolerances (all of the above)

| Item | Value | Clause |
|---|---|---|
| RT before an MH sequence | ≥ 50 ms, or ≥ 20 ms if the RT was preceded by an MH sequence; RT itself optional (figures: "> 50 ms or 0") | 9.10.1 |
| MH bit rate | 600 bit/s ± 0.01 % | V.90 8.2.3.1 |
| MH carrier | analogue 2400 Hz ± 0.01 % at -1 dB, plus 1800 Hz ± 0.01 % guard at -7 dB; digital 1200 Hz ± 0.01 % at nominal | V.90 8.2.3.1 |
| MH sequence length | 40 bits = 66.67 ms | Table 32 |
| Initiator: no response | 2 s + round-trip delay → finish current sequence → retrain or disconnect | 9.10.1.1 |
| Responder: stop response | ANSam detected, or silence detected, or initiating sequence absent 200 ms | 9.10.1.2 |
| Requester after MHack | may continue MHreq ≤ 30 s, or RT, or silence | 9.10.2.1 |
| Requester after MHnack | must send MHcda or MHfrr within 10 s | 9.10.2.1 |
| Held modem: stop MHack | remote RT for 100 ms, or silence for 2 s | 9.10.2.1 |
| Held modem: MHack → ANSam | ANSam within 80 ms (figure: ≤ 80 ms) | 9.10.2.1, Fig. 20 |
| Held modem: hold duration | T1 (10 s … 16 min, or no limit), timed **from the end of the first MHack** | 9.10.2.1, Table 33 |
| MHfrr detected → ANSam | silence up to 80 ms, then ANSam | 9.10.2.1, 9.10.2.3 |
| MHfrr sender → Phase 1 | after ANSam detected for 1 s | 9.10.2.3 |
| MHcda sender (after MHclrd) disconnects | remote RT, or silence, or MHclrd absent 200 ms | 9.10.2.2 |
| Retrain start, for comparison | silence 70 ± 5 ms, then tone; respond after other tone > 50 ms | V.92 9.7 |
| Retrain reversal timing, for comparison | Tone B reversal 40 ± 1 ms after received Tone A reversal, then 10 ms of tone | V.90 9.2.1.1.3 |
| Short Phase 1 after hold | caller: ANSam 1 s → QC1x + CM; answerer: silences 75 ± 5 ms; TONEq and ANSpcm windows 2 s | V.92 9.2 |
| V.8 ANSam normal length | 5 ± 1 s (overridden by T1 while on hold [DERIVED]) | V.8 8.2.2 |

---

## 6. Referenced text the implementer must follow

### 6.1 V.90 8.2.1, 8.2.2 → V.34 10.1.2.1, 10.1.2.2 (Tone A and Tone B)

See section 1.1.

### 6.2 V.90 8.2.3.1 → V.34 10.1.2.3.1, 10.1.2.3.2 (INFO modulation and CRC)

See sections 1.2 and 1.2.4.
- V.90's version swaps V.34's "answer/call" for "analogue/digital", with the same frequencies and levels:
  - analogue modem: 2400 Hz + guard;
  - digital modem: 1200 Hz.

### 6.3 Phase 1 after hold (V.92 9.1, 9.2)

Full Phase 1 is V.90's, i.e. V.8 (9.1).

The **held modem already sends ANSam**, so these short-Phase-1 answer rules apply to it:
- **Held modem is digital (9.2.4.1).**
  - It listens for **QC1a, QC1d or CM**.
  - On **QC1a**: send QCA1d, silence 75 ± 5 ms, then QTS, QTS\ and ANSpcm, and go to 9.2.4.3. In 9.2.4.3, if TONEq is not detected within 2 s of QCA1d, send ANSam and fall back to V.8.
  - On **QC1d**: it **may** take the analogue role (9.2.3.1).
  - On **CM**: normal V.8.
- **Held modem is analogue (9.2.3.1).**
  - It listens for **QC1d, QC1a or CM**.
  - On **QC1d**: send QCA1a, then silence; detect QTS/QTS\ and then ANSpcm (9.2.3.3).
  - On **QC1a**: **may** send QCA1a, silence 75 ± 5 ms, then ANSam (9.2.3.4).
  - On **CM**: V.8.

The **returning modem** is the call modem:
- **Analogue (9.2.1.1).**
  1. After ANSam has been detected for **1 s**, send **QC1a, then CM**.
  2. On QCA1d: stop CM without finishing the octet and go silent, then wait for QTS, QTS\ and ANSpcm.
  3. On QCA1a: stop CM, stay silent until ANSam, then send TONEq.
  4. On JM: V.8.
- **Digital (9.2.2.1).**
  1. After ANSam has been detected for 1 s, send **QC1d, then CM**.
  2. On QCA1a: stop CM, silence 75 ± 5 ms, then QTS, QTS\ and ANSpcm.
  3. If ANSam is detected for 1 s after QC1d, or JM is detected: V.8.

[DERIVED] QC2x/QCA2x follow CRe, not ANSam, so they don't arise in the hold path.

### 6.4 Cleardown from hold by QC (V.92 Tables 2 and 3, rendered pp.14-15)

**QC1a** (V.21(L), 300 bit/s, V.8-style 10-bit frames, sent once and followed immediately by CM):

| Bits | Content |
|---|---|
| 0:9 | ten ones |
| 10:19 | sync `0101010101` |
| 20 | start bit `0` |
| 21 | `0` (analogue modem) |
| 22 | `0` (QC) |
| 23 | P (LAPM) |
| **24:29** | `W0XYZ1`, where WXYZ is **U_QTS**; **`1111` = "Cleardown from on-hold state"** (other codes select QTS Ucodes 61…87) |
| 30:39 | ten ones |
| 40:49 | bits 10:19 repeated |
| 50:59 | bits 20:29 repeated (`000PW0XYZ1`) |

**QC2a** (V.21(H), V.8 bis framing): identification field bits 0:3 `1011`, 4:7 VVVV, **8:11 WXYZ = U_QTS from Table 2**, 12 `0`, 13 P, 14 `0` (QC), 15 `0` (analogue modem).

**QC1d (Table 11, rendered p.21) carries no U_QTS.** Its bits are 21 = 1 (digital), 22 = 0 (QC) and 24:29 = `000LM1`, the ANSpcm level. [DERIVED] So:
- only an **analogue** modem can clear down a held **digital** modem with QC;
- a returning digital modem must use the CM route (6.5) or simply drop the line.

### 6.5 Cleardown from hold by CM (V.8 (11/2000), rendered pp.11, 12, 17, 18)

**CM** carries:
- a call-function octet;
- one or more **modulation-mode octets**: `modn0` has tag b0-b3 = `1010`, b4 = 0, b5 = "PCM availability category present", b6 = V.34 duplex, b7 = V.34 half-duplex; `modn1` and `modn2` are extension octets with b3:b5 = `010` (Table 4);
- optionally a **PCM modem availability** octet `pcm0`: tag `1110`, b4 = 0, b5 = V.90/V.92 analogue, b6 = V.90/V.92 digital, b7 = V.91 (Table 5).

A "cleardown CM" [DERIVED from 9.10.2.1 plus V.8]:
- has **no pcm0 octet**, so modn0 b5 = 0;
- has **every modulation-mode bit** in the modn octets it carries set to 0.

The held modem answers with a JM that:
- has **no pcm0**;
- has the **same number of modulation-mode octets, all modes zero**. This matches V.8 7.4 and 8.2.3 for the "no common modes" case.

V.8 then says:
- the **call DCE may disconnect after sending CJ** (8.1.2);
- the **answer DCE may disconnect on receiving CJ** (8.2.3). V.92 upgrades this to **shall** for the held modem;
- JM is sent **only after at least 2 identical CM sequences** (7.4, 8.2.2);
- JM continues **until all 3 CJ octets** (three all-zero octets with start and stop bits) have been received (8.2.3).

### 6.6 Retrains (V.92 9.7 = V.90 9.5)

These fix the RT tone of each modem (section 1.1) and explain the retrain-versus-hold ambiguity (MOH-9.10.1.1-R5).
- **Initiator:**
  1. Turn OFF 106 and clamp 104 to binary 1.
  2. Send silence for 70 ± 5 ms.
  3. Send its tone: digital B, analogue A.
- **Responder:** acts after **more than 50 ms** of the other tone, following the same silence-then-tone pattern.

### 6.7 Rate sequences with drn (V.92 Tables 23, 24, 30)

See section 3.

---

## 7. Related, but not cited by V.92: V.250 (07/2003) AT control of V.92 hold (rendered pp.91-95, 100)

All of the commands below are **mandatory in V.250 if the DCE implements V.92**.

| Command | Type | Values |
|---|---|---|
| `+PCW=<n>` (6.8.1) | parameter; default 0 | 0 = toggle V.24 circuit 125 and collect Caller ID if enabled by `+VCID`; 1 = hang up; 2 = ignore V.92 call waiting |
| `+PMH=<n>` (6.8.2) | parameter; default 0 | **0 = enable** V.92 modem-on-hold; **1 = disable** |
| `+PMHT=<n>` (6.8.3) | parameter; default not stated | 0 = deny MOH requests; 1…13 = grant with timeout 10 s, 20 s, 30 s, 40 s, 1, 2, 3, 4, 6, 8, 12, 16 min, indefinite. **These equal the Table 33 T1 codes** to put in MHack. |
| `+PMHR` (6.8.4) | action | Starts or confirms a hold request. ERROR if hold is disabled or the DCE is idle. Reply `+PMHR: <v>`, possibly delayed. |
| `+PMHF` (6.8.6) | action | Hook flash while on hold, normally 0.5 s; national rules may change it. ERROR if not on hold. |
| `H` (6.3.6 NOTE) | action | With V.92 hold, the call may be ended without going on-hook. `+PMHF` can then switch the PSTN line. |
| `O0` (6.3.7) | action | Also used to retrain after a hold transaction, or to reconnect to a modem left on hold. |

`+PMHR: <v>` values:
- 0 = denied or not available; the modem may try again later;
- 1…13 = granted, with the same timeout list as `+PMHT`. **These are the received T1 codes**;
- 14 = denied, and later requests will also be denied this session.

`+PMHF` note: the text says the DCE goes on-hook and then "return on-hook", which is surely a typo for "off-hook".

[DERIVED] Mapping MH signals to `+PMHR`:
- received MHack T1 code *c* → `+PMHR: c`;
- MHnack → `+PMHR: 0` (or 14 if the implementation decides the remote will always refuse).

---

## 8. Test ideas [DERIVED]

- **Frame builder and parser:** round-trip the 10 vectors in 1.2.5, and reject:
  - a bad CRC;
  - a wrong sync;
  - an undefined indication (ignore it, NOTE 1);
  - a reserved MHclrd reason (accept as MHclrd with reason unknown).
- **Back-to-back stream:** 3+ MHreq in a row decode as consecutive frames. A receiver joining mid-stream locks on the next `1111 01110010`.
- **Timing:**
  - MHack → ANSam gap ≤ 80 ms after 100 ms of remote RT, and after 2 s of silence;
  - response stops 200 ms after the initiating sequence stops, with sequence completion honoured;
  - the initiator gives up at 2 s + RTD (simulate RTD 0 and 1.5 s);
  - T1 expiry is timed from the end of the first MHack.
- **Retrain-versus-hold race:**
  - digital initiator sends Tone B, then MHreq, while the analogue responder has already sent a Tone A reversal. The analogue modem must end up in hold, not in Phase 2;
  - a genuine retrain must still work: a single B reversal 40 ± 1 ms later, 10 ms of tone, then silence.
- **Cleardown from hold:**
  - QC1a with WXYZ = 1111 → disconnect;
  - CM with no pcm0 and zero modes → JM with zero modes → CJ → disconnect.
- **Cleardown in data mode:** FPE and RR each carrying CPu drn = 0 (bits 21:25 all zero) and CPd drn = 0 (bits 22:26).

---

## 9. Implementation notes

### 9.1 New compared with V.90

- The whole of 8.9 and 9.10 (RT, MH sequences, hold, refusal, cleardown request, fast reconnect) is new. V.90 has nothing like it.
- 9.11 now also allows cleardown via **fast parameter exchange**, and the rate sequences have V.92 names (CPu/CPus/CPd). V.90 allowed only rate renegotiation, with CP/MP.
- Fast reconnect relies on V.92's **short Phase 1** (QC/QCA, 9.2), and on short Phase 2 when that applies (9.4, digested elsewhere).
- Clause 10 is unchanged from V.90.

### 9.2 Reuse in this repo (as read on 2026-09-17)

- `crates/datapump/src/v34/info.rs` already has what MH needs:
  - `FILL`, `SYNC` (`01110010`) and `crc()` (Figure 14, mask 0x8408);
  - `unframe(bits, length)`, which works for length **40**;
  - the private `frame()` produces exactly the Table 32 shape around an 8-bit information field.
  - An `Mh` type is a thin wrapper: indication plus info nibble, each **a pattern sent leftmost first**. Don't use the file's LSB-first `put()` for these nibbles.
- `crates/datapump/src/v34/dpsk.rs` already implements the INFO DPSK, with `Side::Call` (1200 Hz) and `Side::Answer` (2400 Hz, carrier at -1 dB, guard at -7 dB).
  - Use `Call` for the digital modem's MH and `Answer` for the analogue modem's.
  - Its `Side::lengths()` lists the sequence lengths the receiver accepts: INFO0 (49), INFO0d (62), INFO1c (109) and INFO1a (70). A **40-bit MH length** would have to be added for both sides, and MH must be recognised in data mode and retrain states, not only in Phase 2. That module is also where the INFO DPSK receiver lives (`Receiver::feed` → `Option<Info>`).
- The existing Tone A/Tone B generation used for V.90 retrains is exactly RT.
- The existing V.90 retrain responder must grow a "hold aware" branch (MOH-9.10.1.1-R5).

### 9.3 Pitfalls

1. **MH looks like phase reversals.**
   - An MH sequence begins with four ones, which in DPSK is **four 180-degree reversals in a row**, then the sync.
   - A retrain-path reversal detector listening for "a Tone A/B phase reversal" will trigger on it.
   - Run the INFO framer (sync + CRC) alongside the reversal detector during any RT or retrain window, and let a validated MH frame override.
   - A genuine retrain reversal is a single reversal followed by steady tone: 10 ms from the digital modem (V.90 9.2.1.1.3), or 10 ms then L1 from the analogue modem (9.2.2.1.5).
2. **Tones share carriers with MH.** RT and MH use the same carrier (2400 or 1200 Hz), so a steady tone decodes as a string of zeros. Only frame sync plus CRC tells MH from RT. **Detect RT as "carrier present, no valid MH frame for ≥ N ms"**, and never from energy alone.
3. **Tone A guard level.**
   - V.34 10.1.2.1 (1998 text, verified on the render) puts the **guard tone at nominal power** during Tone A.
   - INFO sequences use a guard **7 dB down**.
   - `dpsk.rs` uses -7 dB for both. That is harmless for detection, but it is a literal deviation for RT. Check against captures before changing it.
4. **Bit order of the 4-bit fields.**
   - Clause 8 says patterns go leftmost first, which the vectors above assume.
   - The tempting "LSB-first integer" reading would put bit 12 = 1 in every defined indication. A reversal bug would still self-check (your own encoder and decoder would agree) but would never interoperate.
5. **Sequence completion.**
   - Every stop, switch, or "disconnect on MHcda" must wait for the 40-bit boundary (up to 66.7 ms).
   - Budget that inside the 80 ms MHack→ANSam window. [DERIVED] Worst case: 66.7 ms to finish plus the ANSam start. The 80 ms counts from when MHack stops, so it is still achievable.
6. **200 ms "not detected" is only 3 frames.**
   - On this project's VoIP test lines, concealment inserts of about 20 ms (12 bits) break a frame (memory: *VoIP jitter slips*).
   - Resync on `1111 01110010` after every slip.
   - Don't let a single lost frame look like "absent for 200 ms" unless 200 ms has really passed without a valid frame.
7. **"2 s plus a round-trip delay"** can be about 5 s on the VoIP rig, whose round trip is about 1.5 s each way (memory: *VoIP line round trip*). Use the measured RTDE, not a constant.
8. **ANSam for minutes.** V.8's 5 ± 1 s ANSam cap must not apply in the on-hold state (T1 up to 16 min, or unlimited).
9. **Circuit handling.**
   - 9.10 says nothing about circuits 106, 104 or 109 during hold.
   - [DERIVED] Leaving data mode for RT/MH should behave like a retrain: 106 OFF, 104 clamped to 1.
   - The DTE should see the hold state through V.250 (`+PMHR`, circuit 125 per `+PCW`).
10. **Forget old Phase 1 data.** When the held modem re-enters Phase 1 it must **discard the earlier call's Phase 1 information**: CM/JM contents, LAPM flag, U_QTS and so on.

### 9.4 Ambiguities, apparent errata and open questions

- **Q1.** 9.11 says drn is in **SUVu/SUVd**, but neither has a drn field. The drn fields are in **CPu/CPus (bits 21:25)** and **CPd (bits 22:26)**. This digest assumes the CP sequences are meant, in line with V.90 9.7's "CP ... MP".
- **Q2.** When does the held modem stop MHack?
  - 9.10.1.2: when MHreq has been absent for 200 ms, or on ANSam or silence.
  - 9.10.2.1: after the remote's RT for 100 ms, or silence for 2 s.
  - If the requester stops MHreq and the line then carries neither RT nor clean silence (network announcement, noise), the rules disagree.
  - Proposal: treat 9.10.2.1 as the specific rule for MHack and keep sending MHack until RT or silence, bounded by T1.
  - Or: stop at the earliest of "MHreq absent 200 ms and (RT ≥ 100 ms or silence ≥ 2 s)".
  - Needs a decision.
- **Q3.** MHnack is also an initiating sequence, so its sender's **2 s + RTD** timeout (9.10.1.1) clashes with the requester's **10 s** allowance to choose MHcda or MHfrr (9.10.2.1).
  - Proposal: the MHnack timeout starts only once MHreq is no longer detected.
  - The requester keeps sending MHreq while it decides, as Figures 21 and 22 show.
- **Q4.** The start of the 2 s + RTD timer is not defined: first bit of the initiating sequence? Or the end of the remote's RT? The RTD source is not named either (RTDEd/RTDEa from the last full Phase 2; what if short Phase 2 skipped measuring?).
- **Q5.** In 9.10.1.1, "Tone RT is received" must mean the **remote's** RT, which is the opposite tone per 8.9.1. That is assumed throughout.
- **Q6.** "or an MH response sequence is detected" as an initiation trigger has no worked example. It presumably covers:
  - both modems having started a transaction at once, or
  - the initiator having missed the remote's RT.
- **Q7.** No clause says the requester disconnects after sending **MHcda in reply to MHnack**. Figure 21 implies it.
- **Q8.** Only the **Tone B-initiated** retrain/hold confusion is described. With an analogue initiator (Tone A), the digital responder answers with Tone B, per the retrain rules, and waits for a Tone A reversal. The analogue initiator's MHreq opens with four reversals (pitfall 1). The digital modem needs the same dual detection even though no clause requires it.
- **Q9.** **Glare** (both modems send MHreq, or cross MHclrd with MHreq) is not addressed.
  - Proposal: answer the received initiating sequence per Table 34, and keep sending one's own until 2 s + RTD.
  - Or: the digital modem yields.
  - Needs a decision.
- **Q10.** A **held analogue modem** cannot be cleared down with QC by the returning digital modem, because QC1d has no U_QTS. Only the CM route, T1 expiry or loss of line remain. Confirm this is acceptable.
- **Q11.** Reserved T1 codes (`0000`, `1110`, `1111`) in a received MHack have no defined handling.
  - Proposal: treat as granted with unknown T1. Report `+PMHR` as ... undefined, so choose a policy.
- **Q12.** Which states allow MOH to be initiated? The only stated condition is "circuit 107 asserted". Initiating during Phase 4, rate renegotiation or FPE is therefore not excluded. Probably allow it only from data mode or a retrain start.
- **Q13.** The figures say "> 50 ms or 0" where the text says "at least 50 ms". Figure 24 says "MHffr" (typo).
- **Q14.** With T1 = "no limit" the held modem waits forever. Is there a local cap (DTE `H`, or loss of loop current)?
- **Q15.** V.250 `+PMHT` gives no default. Which value should BinModem use by default: deny (0), or grant with a given T1?
