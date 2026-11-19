# V.ARC

BinModem's own wideband mode. Not an ITU recommendation and not compatible
with anything: both ends have to be BinModem.

Status: the three modulations, the framing and the link layer are built and
run on real sound devices, from the `varc-link` tool and from the modem
itself (`AT+MS=VARC`, `ATD`, `CONNECT .../VARC`, the window's mode box). Not
built yet: the SIP (L16) path. See [What is not done](#what-is-not-done).

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

`station.rs`. Frame kinds: TABLE (2), DATA (3), ACK (4), RESET (5).

**Tables.** There are four marks. Mark 0 is the starting table: 2 bits on
every DMT carrier from 500 Hz to 6 kHz, or 2 bits a symbol (QAM), or 1 bit a
level (PCM).

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

The sound card: the rest of the modem runs at 16 kHz on one channel through
`line::Duplex`. While a V.ARC call is up the window closes that and opens the
same two devices with `line::Wide`, and steps `Modem::varc_step` with the
card's own frames; when the call ends it puts the two-wire line back. A SIP
line refuses a V.ARC call.

During a V.ARC call the waterfall and the spectrum are taken from the left
channel at the card's own rate and drawn from 0 Hz to half of it (24 kHz on a
48 kHz card, 96 kHz at 192 kHz), with the ticks in kilohertz. On every other
call they stay at 0 to 4000 Hz. The oscilloscope and the level meter were
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

- **SIP.** Offering L16 in SDP, a bit-exact tier (16 bits a sample at
  16 kHz, 256 kbit/s, with lost RTP packets as erasures) and fallback to
  G.711. The Reed-Solomon decoder already takes erasures for this.
- **Scopes for it.** The window has no constellation, SNR-by-carrier or bit
  table display for V.ARC; `varc-link --csv` is the only way to see those.
- **The window beyond a loop.** The window has run V.ARC/DMT calls to itself
  (`ATDLOOP`) through VB-Cable A and through the Realtek lead. Choosing it
  from the mode box rather than typing `AT+MS`, the QAM and PCM modes in the
  window, and a file or PPP over it have not been tried. The headless tools
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

`--modem` drives the lead through a whole modem as the window does
(`AT+MS=VARC`, `ATDLOOP`) and checks what comes back.
`--loop` needs each output channel joined to the same input channel.
`--oneway` sends on both channels and returns the tables and
acknowledgements inside the program. `--csv` writes SNR and bits per carrier.
`--gap` and `--level` override the 8 dB gap and the transmit level.
