# V.ARC

BinModem's own wideband mode. Not an ITU recommendation and not compatible
with anything: both ends have to be BinModem.

Status: built and run on real sound devices, from the `varc-link` tool and
from the modem itself (`AT+MS=VARC`, `ATD`, `CONNECT .../VARC`, the window's
mode box), and over a SIP call between two of these on one machine. See
[What is not done](#what-is-not-done) for what has not been tried.

Code: `crates/varc`. Line: `crates/line/src/wide.rs`. The call:
`crates/modem/src/varc.rs`. Tool: `crates/modem/src/bin/varc-link.rs`.

![The window on a V.ARC/DMT call to itself through a real lead: Realtek line out to line in at 192 kHz, both channels](../images/v-arc/window-loop.png)

## What it is for

Every other mode here is built for a telephone channel: 3 kHz wide, about
35 dB of SNR, 64 kbit/s of G.711 underneath at best. V.ARC is for the channel
a sound card gives when it is wired to another sound card: 20 kHz wide or
more, 60 to 90 dB of SNR, and four wires (an output and an input each way),
so there is no echo to cancel.

Two ends, full duplex, each direction on its own wire. A stereo lead is two
wires, used either as one each way (one machine, a loopback) or both the same
way (two machines, a lead each way), in which case the two are bonded into
one octet stream.

## The three modulations

| | V.ARC/DMT | V.ARC/QAM | V.ARC/PCM |
|---|---|---|---|
| Kind | multicarrier, bit-loaded per carrier | single carrier | baseband levels |
| Symbol rate | 44.1 symbols/s of up to 2047 carriers | 16 000 baud | 16 000 baud |
| Band at 48 kHz | 47 Hz to 20.6 kHz | 0.9 to 20.1 kHz | DC to 9.6 kHz |
| Bits | 0 to 24 per carrier | 2 to 24 per symbol | 1 to 12 per level |
| Sample rates | any (plan follows the rate) | multiples of 48 kHz | multiples of 48 kHz |
| Module | `dmt.rs` | `sc.rs` | `sc.rs` |

All three share one framing layer, one link layer and one receiver front end
(a resampler steered by the timing loop).

### V.ARC/DMT

- Real baseband signal, no carrier, so no frequency offset between ends: only
  the difference in sample clocks.
- FFT size `n` by sample rate: 512 below 30 kHz, 1024 to 60 kHz, 2048 to
  120 kHz, 4096 above. Guard (cyclic prefix) `n/16`. Carriers are 31 to 47 Hz
  apart and a symbol is about 23 ms at every rate.
- Superframe: 1 sync symbol + 32 data symbols (0.75 s at 48 kHz).
- Sync symbol: a known QPSK point on every carrier. One of four sequences;
  which one says which bit table the following 32 symbols use.
- Data symbol: a carrier with `b > 0` bits carries a Gray-coded QAM point
  (rectangular when `b` is odd). A carrier with 0 bits, and every carrier
  `k` with `k mod 32 = 16` (the pilots), carries a known QPSK point.
- Carriers above 0.86 of Nyquist are never loaded. The resampler is flat to
  0.87 of Nyquist; above that its response moves as the clocks slide.
- Transmit level -16 dBFS RMS.

Receiver, in order:

1. **Hunt.** Correlate the guard against the end of the body it copies,
   folded over one symbol span for 8 symbols.
2. **Frame.** Transform each symbol and compare neighbouring carriers with
   the sync sequences (differential, so no channel estimate is needed).
3. **Place the window.** The channel's delay from the first estimate moves
   the transform to start a third of a guard before the signal.
4. **Train.** 44 symbols to settle, 88 to measure noise on each carrier
   against the known points (2 s at 48 kHz, about +-0.5 dB per carrier).
5. **Load.** `b = floor(log2(1 + SNR / gap))`, gap 8 dB, capped at 24.

Running, per symbol: timing error from a weighted fit of phase against
carrier number over the known carriers; that error is taken out of the
symbol at once and also drives a second-order loop (about 1 Hz) on the
resampler. One complex tap per carrier, adapted by LMS.

### V.ARC/QAM and V.ARC/PCM

One machine. PCM is QAM with the carrier at 0 Hz and the quadrature axis
empty.

- 16 000 baud, root raised cosine, 20% excess bandwidth, 24 symbols either
  side under a Kaiser window (beta 8).
- QAM carrier 10.5 kHz.
- Superframe 4096 symbols (0.256 s): a 128-symbol sync word, then 16 groups
  of 248 data symbols.
- Sync word: four quarters of 32 known symbols. The second and third
  quarters are inverted by the two bits of the table mark, the fourth by
  both.
- PCM only: symbols go in eights. The first of each eight gives up its sign
  bit as a polarity flag, and the eight are sent whichever way up brings the
  running sum of all levels sent back towards zero. That keeps DC out of a
  line that will not pass it. Cost: 1 bit in `8b`.
- Transmit level -14 dBFS RMS.

Receiver: resampler, mix to baseband (QAM), matched filter read three times
a symbol, a 121-tap equaliser at T/3 (NLMS, decision-directed), a
second-order phase loop (QAM) or a baseline tracker (PCM), slicer.

The sync word is read **ahead of the equaliser**, by correlation, for three
things: whether the signal is still there, which mark it carries, and timing
(a parabola through the correlation at the expected sample and its
neighbours). The equaliser's own output of the sync word is used for one
thing: to turn the constellation the right way up, since decision-directed
loops are equally happy a quarter turn round.

Loading: `b = floor(log2(1 + SNR/gap))` for QAM, half that for PCM.

## Probing the line: the starting table

Both ends of a V.ARC/DMT call start on a table neither has had to be told,
and the first table made to measure has to be carried over it. So it has to
work on a line nothing is known about.

It used to be two bits on every carrier from 500 Hz to 6 kHz, each carrier
for itself. That wants the whole of that band and a dozen decibels on it. A
line that passed 300 to 3400 Hz, or only the top of the band, or was noisy,
never got as far as being measured.

The starting table now chooses no stretch of the band (`Plan::spread`):

- A symbol carries 48 four-point symbols, 96 bits.
- Every carrier from the bottom of the band to the top that is not a pilot
  carries one of them, round and round: 9 copies of each at 48 kHz, 36 at
  192 kHz.
- Each copy is turned by its carrier's known point, so that a symbol sent
  dozens of times over is not a row of spikes.
- The receiver adds the copies together, each weighted by the noise heard on
  its carrier over the last few symbols, and slices the sum.
- What was sliced is then every carrier's right answer. Every carrier's noise
  is measured against it, the dead ones included, and that measurement is the
  probe: the table made from it puts bits where the line is and none where it
  is not.
- Timing is taken twice a symbol: roughly from the pilots, then from every
  carrier once the first pass has said what each was sent. A narrow line may
  have two pilots in it.

Any 48 carriers in a row that work are enough, which is 2.3 kHz, wherever in
the band they are. Tested in simulation at 48 kHz, where the figure is a
carrier's signal against its noise:

| Line | Result |
|---|---|
| 300 to 3400 Hz, 40 dB | starts, loads only inside the band |
| 12 to 16 kHz only, 40 dB | starts, loads only inside the band |
| Whole band, 3 dB | starts |
| Whole band, 0 dB | starts after losing a few blocks at the beginning |
| 300 to 3400 Hz, 15 dB | starts |
| 300 to 3400 Hz, 10 dB | found, cannot be read |
| 300 to 3400 Hz, under 10 dB | not found: the sync symbol is not heard |

What it costs is speed before the first table: 3.4 kbit/s where there was
about 9. On the Realtek lead at 192 kHz the made-to-measure table is in use
about 7 s after the start.

It also measures a leak for what it is. Two ends with the same known points
used to measure a perfect line, because what leaked across was a faint copy
of what each expected. Now nearly every carrier carries data while the line
is measured, and the two ends' data differs.

V.ARC/QAM and V.ARC/PCM have no such thing. Their band is fixed, 1.5 to
19.5 kHz and 0 to 9.6 kHz, and a line that does not pass it does not carry
them.

## What the far end hears

A transmitter cannot hear its own line. It does not know whether what it
sends arrives faint, or so loud that the far converter is flat against its
stops. So each end measures what arrives on each channel, a second at a
time, and tells the other in a frame of its own (REPORT, 8 octets):

| Field | What it is |
|---|---|
| Level | RMS of what arrived, in half decibels below full scale |
| Peak | the largest sample in the second, the same way |
| Clipped | samples in the second at full scale |
| Es/N0 | signal to noise at the slicer over the band in use, in tenths of a decibel; nought until measured |
| Trim | how far the reporting end has turned itself down |

Reports are sent whether or not the reporting end can read anything, since
the end that most needs telling is the one deafening the other.

**Acting on it.** Samples at full scale are the one fault no table loads
around: the tops of the waveform are gone on every carrier at once. So:

- If the far end reports samples at full scale, or peaks within 1 dB of it,
  this end sends lower: by enough to put the peaks at -4 dBFS, and 3 or 9 dB
  more if any were clipped, since a clipped peak reads as full scale however
  far over it was.
- If it is trimmed and later hears that the peaks are below -14 dBFS, some
  comes back off.
- It never sends above the level it was made for. That level is already as
  much as its own converter has room for (-16 dBFS RMS for the multicarrier
  mode, whose peaks are 10 to 12 dB above that).
- A change of level moves everything the far receiver has learned. So each
  one puts the line back on the starting table and tells the far end to
  measure afresh (LEVEL), and changes are kept 3 s apart.

In simulation, 20 dB of gain into a converter that clips: the sending end
ends up 26 dB down, nothing is clipped, and the stream arrives whole.

**Seeing it.** The window has a line for it while a V.ARC call is up, and the
transcript says when the level changes. On the Realtek lead:

![What the far end hears](../images/v-arc/far-end.png)

What this cannot do is turn the far end's input down, or this end's output
up past its own full scale: those are the sound card's mixer. It says which
is needed.

## Modulation and coding

There is no list of modes to pick from. The receiver measures the line and
asks for a number of bits; the far end adopts it.

| | V.ARC/DMT | V.ARC/QAM | V.ARC/PCM | V.ARC/L16 |
|---|---|---|---|---|
| Symbols | carriers 46.9 Hz apart (31.3 at 16 kHz), up to 0.86 of half the sample rate | 16 000 baud on a 10.5 kHz carrier | 16 000 baud at baseband | none: the samples |
| Bits | 0 to 24 on each carrier, each for itself | 2 to 24 a symbol | 1 to 12 a symbol | 16 a sample |
| Grid | square for an even count, a 2:1 rectangle for an odd one | the same | a ladder of levels | - |
| Code, starting | Reed-Solomon 160/192, rate 0.83 | 216/248, rate 0.87 | 185/217, rate 0.85 | - |
| Code, loaded | 16 check octets a word, about 228/244, rate 0.93 | 16 check octets, rate 0.89 to 0.94 | the same | 48 check octets |
| Net rate, starting | 3.4 kbit/s (2.3 at 16 kHz) | 27 kbit/s | 11.6 kbit/s | 200 kbit/s |

A bit more on a carrier or a symbol wants about 3 dB more signal to noise,
and there are 8 dB kept in hand: `bits = floor(log2(1 + SNR / 6.3))`, so
about `3 x bits + 8` dB. For PCM it is half as many bits for the same ratio.

| Bits | Grid | SNR wanted | V.ARC/QAM net | V.ARC/DMT net at 48 kHz, every carrier alike |
|---|---|---|---|---|
| 2 | 4 | 13 dB | 29 kbit/s | 34 kbit/s |
| 4 | 16 | 20 dB | 58 kbit/s | 67 kbit/s |
| 6 | 64 | 26 dB | 85 kbit/s | 101 kbit/s |
| 8 | 256 | 32 dB | 116 kbit/s | 135 kbit/s |
| 10 | 1024 | 38 dB | 139 kbit/s | 169 kbit/s |
| 12 | 4096 | 44 dB | 170 kbit/s | 202 kbit/s |
| 16 | 65 536 | 56 dB | 232 kbit/s | 270 kbit/s |
| 20 | 1 048 576 | 68 dB | 286 kbit/s | 337 kbit/s |
| 24 | 16 777 216 | 80 dB | 348 kbit/s | 405 kbit/s |

The DMT column is two, four and so on times that at 96 and 192 kHz, since
there are that many more carriers: 1.64 Mbit/s at 192 kHz with 24 bits on
every one. A real line gives each carrier its own count and the rate is
whatever they add up to.

The code does not change with the line. Retransmission covers what it does
not correct, and a table that turns out too high is asked for again lower.

## Over a SIP call: V.ARC/L16

A fourth way of running, which is no modulation at all (`exact.rs`).

An account with `linear = yes` offers `L16/16000` in its SDP ahead of PCMU
and PCMA, and takes it when offered: sixteen-bit linear samples, two octets
each, at 16 kHz (RFC 3551 4.5.11). Between two such accounts the call's
samples arrive as the sixteen bits that were sent, so the samples are the
data.

- Superframe: a sync word of 64 known samples, then a block of 2000 samples
  that are 4000 octets. Eight a second.
- FEC: Reed-Solomon with 48 check octets a codeword.
- A lost RTP packet is 320 samples of silence where data should be. Scrambled
  data is never silence, so a run of 8 or more samples that are exactly zero
  is taken as a hole and its octets are given to the decoder as erasures. One
  lost packet in a block is 40 erasures a codeword and is rebuilt; two is not,
  and the block is resent.
- Rate: 256 kbit/s of samples, 200 434 bit/s of data.

![The window as the caller on a V.ARC/L16 call over SIP](../images/v-arc/window-sip.png)

What the call turns out to be decides what runs on it:

| The call | What `AT+MS=VARC` becomes |
|---|---|
| L16, and the samples arrive exactly | V.ARC/L16 |
| L16, but nothing is heard in 4 s, or the two ends have not agreed in 10 s | V.ARC/DMT at the call's 16 kHz |
| G.711, because the far end answered with it | an ordinary call from V.8, with `+MS` set to `V92` |

`VARCQ` and `VARCP` do not run on a SIP call: they need a multiple of 48 kHz.

## Octets on the line

Same for all three (`frame.rs`). Each modulation offers room for so many
octets per *block*: one DMT symbol or one group of 248 symbols on a loaded
line, more on the starting table, always at least 200 octets.

| Layer | What |
|---|---|
| Scrambler | x^15 + x^14 + 1, restarted every block |
| FEC | Reed-Solomon over GF(256), 16 check octets (32 on the starting table). The block is split into the fewest codewords of nearly equal length and sent a column at a time |
| Header | 3 octets: a count, and a pointer to the first frame that starts in this block |
| Frames | kind (1), length (2), body, CRC-32 (4). A frame may run on into the next block |
| Idle | a zero where a kind should be, then pseudo-random filler |

A block is on the line about 23 ms (DMT) or 16 ms (QAM, PCM) after its
octets were offered, on a loaded line.

## The link

`station.rs`. Frame kinds: TABLE (2), DATA (3), ACK (4), RESET (5), REPORT (6), LEVEL (7).

**Tables.** There are four marks. Mark 0 is the starting table: for DMT, 48
four-point symbols each sent on every carrier that has its turn (see
[Probing the line](#probing-the-line-the-starting-table)); 2 bits a symbol
(QAM); or 1 bit a level (PCM).

1. Each receiver measures its line and works out a table.
2. It gives the table the next mark (1, 2, 3, 1, ...) and sends it back the
   other way in a TABLE frame, in every block.
3. The far transmitter switches at its next superframe and marks the sync
   symbol or word.
4. The receiver sees the mark and stops sending the table. There is no other
   acknowledgement.

Recovery:

| Seen by a receiver | What it does |
|---|---|
| Uncorrectable codewords now and then | Proposes a new table with 1.5 dB more gap |
| Nearly every codeword bad for 1 s | Puts its *own* transmitter back on mark 0, asks the far end for mark 0 (RESET), and measures again with 3 dB more gap |
| A mark it has no table for | Asks for mark 0 (RESET) |

The second row is for both directions failing together, when neither end can
tell the other anything on a loaded table.

**Retransmission.** Selective repeat. DATA frames carry a 32-bit number and up
to 1024 octets; ACK carries the next number expected and a bitmap of what has
arrived beyond it. A frame is sent again after twice the measured round trip,
or after one round trip if the far end is seen to have a later frame.
Window 512 frames.

**Bonding.** A station has one channel per wire. Each has its own tables.
DATA frames are dealt to whichever channel has room next and put back in
order by number.

**Colours.** Each wire in a lead uses its own set of known points. See the
first finding below.

## In the modem

| | |
|---|---|
| Choosing it | `AT+MS=VARC` (multicarrier), `AT+MS=VARCQ` (QAM), `AT+MS=VARCP` (PCM), or the window's mode box |
| Calling | `ATD` with any number at one end, `ATA` at the other. No V.8, no tones |
| A lead that comes straight back | `ATDLOOP`: the modem listens for its own signal and what is typed comes back |
| Connected | `CONNECT <rate>/VARC`, the rate being what arrives, in bit/s of data, over the channels that loaded |
| A lead with one channel wired | connects on that one after waiting 6 s for the other |
| Nobody there | `NO ANSWER` after 60 s |
| The far end gone | `NO CARRIER` after 5 s of hearing nothing |
| `+++`, `ATH`, `ATO` | as on any call |
| Error control | none of V.42's; the station's own retransmission. `+ES`, `+DS` are not consulted |

The two ends differ in which known points they send, so one must dial and
the other answer.

On a SIP line the same commands are used and the account has to say
`linear = yes`. The call is placed and answered as any SIP call is.

The sound card: the rest of the modem runs at 16 kHz on one channel through
`line::Duplex`. While a V.ARC call is up the window closes that and opens the
same two devices with `line::Wide`, and steps `Modem::varc_step` with the
card's own frames; when the call ends it puts the two-wire line back.

The two devices need not run at the same rate. A line can be a lead one way
and a virtual cable the other, at 192 kHz and 48 kHz, and then each direction
is a line of its own: the receiver is built for the rate of the device it
hears on and the transmitter for the rate of the one it sends on. What is
heard is stepped as it arrives (`Modem::varc_hear`) and what is sent is made
as fast as the sending device takes it (`Modem::varc_say`), since two
devices are two clocks. Below, two windows on one machine: Line In at 192 kHz
and CABLE-B at 48 kHz, 2.14 Mbit/s one way and 0.80 the other.

![Two windows, each direction at its own rate](../images/v-arc/two-rates.png)

Before this a V.ARC call on two such devices ended at once in `NO CARRIER`,
with the reason only in the transcript.

During a V.ARC call the waterfall and the spectrum are taken from the left
channel at the card's own rate and drawn from 0 Hz to half of it (24 kHz on a
48 kHz card, 96 kHz at 192 kHz), with the ticks in kilohertz. On every other
call they stay at 0 to 4000 Hz, except a V.ARC call on a SIP line, which is
drawn to the call's 8 kHz.

The symbol diagram shows what the receiver sliced, on the first channel that
is hearing anything, scaled so the grid's outermost point is at the edge of
the box:

![The symbol diagram in the three modes](../images/v-arc/symbols.png)

- **V.ARC/QAM:** every symbol.
- **V.ARC/PCM:** each level against the one before, as V.90's is drawn,
  since levels have no plane to be points on.
- **V.ARC/DMT:** the carriers differ in how many bits they carry, and a grid
  of four points laid over one of a thousand shows neither. So it shows the
  carriers of the commonest loading, about sixty of them a symbol. Until a
  table is loaded it shows the four known points.
- **V.ARC/L16:** nothing. There are no symbols.

The panel's label gives the size of the grid shown. On a good lead that is
tens of thousands of points or more, which no screen resolves: what is drawn
is a filled box, sharp at the edges when the line is clean. The picture is
the Realtek lead: 32 768 points on the commonest carriers, 1 048 576 for the
single carrier, 64 levels.

Beside the size of the grid the panel gives the code on what is arriving,
as data octets over octets a codeword (`code 236/252`), and the error control
line spells it out. It follows the table, so it changes when the two ends
change tables.

![The code rate beside the grid's size](../images/v-arc/code-rate.png)

The oscilloscope and the level meter were
built for the modem's 16 kHz and are still shown the left channel cut down to
that without filtering, which shows that something is there and measures
nothing.

## Measured

All with `varc-link --loop`: two ends in one program, one sending on the left
channel and one on the right, each loading the other's line. Rates are the
net rate of the table in use (frames, after FEC), per direction. "Wrong
octets" is against a known stream at the far end.

| Path | Rate | Mode | SNR | Net rate each way | Wrong octets |
|---|---|---|---|---|---|
| Realtek, same lead, through the modem (`--modem`: `ATDLOOP`, both channels one way) | 192 kHz | DMT | 54 / 59 dB | 2146 kbit/s | 0 of 7.7 M |
| SIP between two `modem-sip` on this machine, L16 | 16 kHz | L16 | exact | 200 / 200 kbit/s | a line of text only |
| VB-Cable A | 48 kHz | DMT | 80 dB | 400 / 399 kbit/s | 0 of 699 k |
| VB-Cable A | 48 kHz | QAM | 70 / 80 dB | 240 / 232 kbit/s | 0 of 607 k |
| VB-Cable A | 48 kHz | PCM | 62 / 76 dB | 122 / 83 kbit/s | 0 of 298 k |
| Realtek line out to line in, real lead | 192 kHz | DMT | 54 / 60 dB | 1132 / 1017 kbit/s | 0 of 4.8 M |
| Realtek, same lead | 192 kHz | QAM | 64 / 68 dB | 201 / 201 kbit/s | 0 of 489 k |
| Realtek, same lead | 192 kHz | PCM | 47 / 47 dB | 83 / 83 kbit/s | 0 of 198 k |

Notes on those numbers:

- The VB-Cable DMT row was run before the colours change and has not been
  repeated since.
- The Realtek rows had uncorrectable codewords in the QAM run (12 of about
  2600 each way) and the PCM run (6 of about 1200). All were retransmitted.
  The DMT run had none in about 20 000.
- Both ends share one clock in every row (one device), so the clock recovery
  is exercised only in simulation, to +-150 ppm.
- The SIP row is two processes calling each other over the loopback
  interface: real SIP, SDP and RTP, no network and no lost packets. Both ends
  said `CONNECT 200434/VARC` half a second after the answer and one line of
  text crossed. No bulk transfer was measured on it. Erasure decoding of lost
  packets is tested in simulation only.
- The 192 kHz DMT figure uses carriers up to 82 kHz. SNR on the Realtek falls
  from about 66 dB below 48 kHz to about 51 dB at 78 kHz.
- SNR for the single-carrier rows is at the slicer. For DMT it is the mean
  over the loaded band.

### What the real lead found

Three faults that simulation with one clean wire each way did not show:

1. **Crosstalk measured as signal.** Both ends sent the same known points, in
   step. The leak from one channel into the other was then a faint copy of
   what each receiver expected, and measured as line gain. Once data started
   the leak was noise, about 66 dB down on the Realtek, on a table chosen for
   72 dB. Every block failed. Fix: each wire has its own known points.
2. **Deadlock.** With both directions failing, neither end could ask the
   other for a lower table. Fix: the second recovery row above.
3. **An idle line repeating itself.** Idle blocks were identical, so the leak
   between two idle channels was a fixed offset per carrier and the running
   noise estimate drifted. Fix: pseudo-random filler.

## Simulation tests

`cargo test -p varc --release` (36 tests). The ones that say most:

- DMT: measured SNR within 1 dB of the SNR set, at 30, 50 and 70 dB; 300 k
  octets exact at 55 dB with the clocks 0, +60 and -120 ppm apart.
- Station: a 0.2 s cut costs time and no octets; an end restarted mid-link
  recovers; a line that gets 6 dB noisier is reloaded lower; two channels at
  60 and 35 dB are loaded separately and bonded.
- QAM: SNR and table as predicted at 45 and 62 dB, clocks to 70 ppm.
- PCM: exact through a 10 Hz high-pass at 40 and 55 dB.
- Reed-Solomon: errors and erasures to the bound for 2 to 32 check octets.

## What is not done

- **SIP beyond this machine.** Never run over a network, through a PBX, or
  with packets really lost. Whether any PBX passes L16 at 16 kHz untouched
  is not known.
- **The G.711 fallback, to a connection.** A V.ARC call answered in G.711
  does carry on as an ordinary call, and between two `modem-sip` on this
  machine it trained to V.90 data and then never said `CONNECT`. A plain
  `AT+MS=V92` call between the same two does exactly the same, and a plain
  `AT+MS=V34` one ends in `NO CARRIER`, on `main` as well as here, so that is
  not V.ARC's; but it means the fallback has not been seen to end in a
  connection.
- **The probe on a real poor line.** The starting table is tested on poor
  lines in simulation only. The only real lines so far are a lead and a
  virtual cable, both good.
- **Recovery from a gap in the sound.** A gap in what the sound card sends
  cost a call seconds, in both the multicarrier and the single-carrier
  modes; half the blocks bad until the tables were made again. The gaps
  seen were the output running dry at 192 kHz, which a cushion of 40 ms
  where there were 10 has stopped on this machine. What a gap does once it
  happens is unchanged.
- **Scopes for it.** The window has the symbol diagram, and no
  SNR-by-carrier or bit table display for V.ARC; `varc-link --csv` is the
  only way to see those. The multicarrier diagram shows one loading of many.
- **The window beyond a loop.** The window has run V.ARC/DMT calls to itself
  (`ATDLOOP`) through VB-Cable A and through the Realtek lead. Choosing it
  from the mode box rather than typing `AT+MS`, and a file or PPP over it,
  have not been tried. The QAM and PCM modes have run in the window as loop
  calls on the Realtek lead. The headless tools
  (`modem-sip`, `--answer`) do not know about V.ARC.
- **Two machines.** Never run between two sound cards with separate clocks.
- **44.1 kHz** for the single-carrier modes (they need a multiple of 48 kHz).
- **Compression** on top.

## Running it

```text
varc-link --list
varc-link --in "CABLE-A Output" --out "CABLE-A Input" --loop
varc-link --in "Line In" --out "Speakers" --loop --mode qam --seconds 60
varc-link --in "Line In" --out "Speakers" --oneway --csv snr.csv
varc-link --in "Line In" --out "Speakers" --modem
```

Over SIP, with two accounts that have `linear = yes` and point at each
other:

```text
modem-sip --accounts FILE --account host --answer --type AT+MS=VARC
modem-sip --accounts FILE --account caller --type AT+MS=VARC --type ATD100 --send hello
```

`--modem` drives the lead through a whole modem as the window does
(`AT+MS=VARC`, `ATDLOOP`) and checks what comes back.
`--loop` needs each output channel joined to the same input channel.
`--oneway` sends on both channels and returns the tables and
acknowledgements inside the program. `--csv` writes SNR and bits per carrier.
`--gap` and `--level` override the 8 dB gap and the transmit level.
