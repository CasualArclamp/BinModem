# What works

[Back to the README](../../README.md)

This page gives the status of the modem modulations and the full table of
functions. The other pages of the manual are in the
[README](../../README.md#manual).

## Real calls

BinModem places and answers real calls. Every modulation in the
[table of functions](#functions) has connected to real modems and to the
servers of providers over a VoIP trunk. The range is from Bell 103 to V.90.

Full interactive sessions on public dial-up gateways have crossed it with V.42
error control and V.42bis compression. Each byte was correct, the ANSI
included.

Some far ends answer no XID and announce compression in band. BinModem follows
that too. A test vector keeps 1957 octets of the screen of one board.

No far end answered an XID in any of twelve recorded calls. A replay of those
calls through the whole modem showed the cause:

- Our XID broke four rules of the Recommendation at the same time.
- The worst error put a length before the V.44 offer, where 12.2.1.3 says
  there is none.
- Thus no far end could read the offer.
- BinModem also could not read a conformant answer to the offer.

All four errors are fixed.

## V.32bis

V.32bis operates up to 14 400. One trellis code serves 7200, 9600, 12 000 and
14 400. Figure 1/V.32bis shows the code one time, with four parallel lines.
Each line has a label with the rates that it exists for.

Only the number of
bits that go through without a change is different, from one to four. The
redundant bit gives nine decibels at each rate. That is why 14 400 fits in the
same channel as 4800.

## V.34

V.34 works on real calls. Through a VoIP trunk to real modems it has connected
at 28 800 to 33 600. It carried login banners and PPP without an error.

The recordings of those calls replay through the modem and give the same
banners. In the replays the modem answers the rate renegotiation of the far
end. It also follows the slips of the jitter buffer. It does both in the
start-up and after the connection.

Phase 2 reads the probe of the far end for 300 ms, not the full 500 that V.34
permits. A real modem needed the difference to hear our reply. Without the
difference that modem stalled three times in sequence.

## V.90

V.90 works in both directions.

The analogue modem is the half that dials a provider. It reads the start-up of
a real server as the Recommendation writes it: CM and JM, INFO0d and INFO1d,
Ja, TRN1d, Jd and the DIL. It connects in PCM to the real servers of providers
over the VoIP trunk.

The digital modem is the half that a provider operates. It answers too, so two
BinModems connect at full PCM rates over a virtual cable.

The tests use a simulated G.711 network with these impairments:

- A-law and μ-law
- a robbed bit
- a pad
- noise
- a round trip of one second
- a sound card with an error of 120 ppm
- a jitter buffer that slips every few seconds
- the gain control of a softphone

Against that network, BinModem does these things:

- It connects at 52 000 to 56 000.
- It renegotiates and retrains from each end.
- It detects a far end that has hung up.
- It falls back to V.34 on a route that would carry less as PCM.

A codec or a VoIP path can cut the top of the band. Then the analogue modem
asks for spectral shaping, as a real modem does. The server then sends almost
nothing where the cut is.

## How the constellations were read

Every constellation was read from the figures by position, not from extracted
text. Extracted text loses the sign of each coordinate and mixes the axis
labels through the rows.

[tools/read_constellation.py](../../tools/read_constellation.py) takes the
scale from the distance between the ticks. It takes the origin from the
quarter-turn symmetry of the constellation. For V.32bis Figure 2-3 it gives
exactly the thirty-two points that were read by hand from Figure 3 of V.32.
Those points had three other checks. The result is two documents, two methods
and one table.

## Functions

| | |
|---|---|
| **Modulations** | Bell 103 (300), Bell 212A (1200, with a fallback to 103), Bell 202 (1200 half duplex), V.21 (300), V.22 (1200), V.22bis (1200/2400), V.32 (4800/9600, both codings), V.32bis (7200/12000/14400), V.34 (4800 to 33600: shell mapping, 16/32/64-state 4D trellis codes, precoding, non-linear encoding), V.90 analogue and digital modems (28000 to 56000 PCM down, V.34 up: DIL analysis, modulus encoding, spectral shaping, rate renegotiation. The digital modem answers through a softphone), U.S. Robotics x2 as a server (32000 to 56000 PCM down, V.34 up, with the rate changes of the client followed) |
| **Negotiation** | V.8 CM/JM/CI/CJ and ANSam. The V.25 answer tone is told apart from ANSam |
| **Error control** | V.42 LAPM: detection, HDLC, XID, REJ/SREJ, mod-128 |
| **Compression** | V.44 (LZJH) and V.42bis (BTLZ), both offered in one XID, and the far end selects one. V.42bis is also followed in band |
| **Commands** | V.250 AT: `+MS`, `+ES`, `+DS`, `+DS44`, `+ER`, `+DR`, S-registers (S70: x2 on offer, our own), `+++` |
| **Terminal** | ANSI/CP437 with mouse reports. Telnet (RFC 854) lets you use the terminal alone |
| **Settings** | kept between runs, so the modem starts again in the state in which you left it |
| **Files** | ZMODEM send and receive |
| **Fax** | T.30 group 3, send and receive. V.17 (7200 to 14 400), V.29 (4800/7200/9600) and V.27 ter (2400/4800). Super G3: V.8, then T.30 Annex F on the half-duplex mode of V.34 at up to 33 600. T.4 Modified Huffman and Modified READ, T.6 MMR, T.85 JBIG. T.30 Annex A error correction mode. Any number of pages in a call, each drawn as it arrives. A PDF is sent one page to a sheet at its printed size. The received pages are saved as a PDF |
| **Network** | PPP (RFC 1661/1662) with LCP, PAP and CHAP, and IPCP, Van Jacobson header compression (RFC 1144), and a ping over it. A dial-in login prompt, and a login script for calls that BinModem places |
| **Internet** | our own TCP (RFC 9293) and an HTTP/HTTPS proxy (RFC 9112): pages directly to the internet through a provider, or through a far BinModem that has it |
| **Line** | full-duplex sound card, or a WAV to replay |

## Portability

BinModem is portable in principle. It uses `cpal` for the audio and `eframe`
for the window, and it has no OS-specific code. But until now it was built and
run only on Windows.
