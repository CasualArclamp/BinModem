# BinModem

<img width="1319" height="1007" alt="dialupmodem2_0IgsyArLo3" src="https://github.com/user-attachments/assets/99c3c5a9-0a1f-4c6d-af22-05f37344e7d3" />

A dial-up softmodem in Rust. Everything a modem does — the tones, the
handshakes, the error control, the compression — is code, and the line is a
sound card. No DSP chip, no driver blob, and none of a winmodem's dependence on
one vendor's Windows.

Written against the ITU-T Recommendations, with clause numbers cited in the
source for every normative constant, and against the RFCs for everything
carried over the top of them. 924 tests.

## What works

It places and answers real calls. Full interactive sessions — public dial-up
gateways, reached over a VoIP trunk — have crossed it at V.22bis 2400 and at
V.32 4800, with V.42 error control and V.42bis compression, byte for byte
correct including the ANSI. Where a far end answers no XID at all and simply
announces compression in band, that is followed too, and 1957 octets of one
board's screen are kept as a test vector.

V.32bis runs to 14 400. One trellis code serves 7200, 9600, 12 000 and
14 400 — Figure 1/V.32bis draws it once, with four parallel lines each
labelled by the rates it exists for — and only the number of bits riding
through untouched changes, from one to four. The redundant bit buys nine
decibels at every one of them, which is why 14 400 fits in the channel 4800
does.

Every constellation was read off the figures by position rather than out of
extracted text, which loses the sign of each coordinate and shuffles the axis
labels through the rows. [tools/read_constellation.py](tools/read_constellation.py)
takes the scale from the spacing of the ticks and the origin from the
constellation's own quarter-turn symmetry. Run on V.32bis Figure 2-3 it gives
back exactly the thirty-two points read off V.32's Figure 3 by hand, which had
been checked three other ways: two documents, two methods, one table.

| | |
|---|---|
| **Modulations** | Bell 103 (300), V.22 (1200), V.22bis (1200/2400), V.32 (4800/9600, both codings), V.32bis (7200/12000/14400) |
| **Negotiation** | V.8 CM/JM/CI/CJ and ANSam; V.25 answer tone told apart from it |
| **Error control** | V.42 LAPM — detection, HDLC, XID, REJ/SREJ, mod-128 |
| **Compression** | V.42bis, negotiated in XID or followed in band |
| **Commands** | V.250 AT: `+MS`, `+ES`, `+DS`, `+ER`, `+DR`, S-registers, `+++` |
| **Terminal** | ANSI/CP437 with mouse reporting; telnet (RFC 854) to use it alone |
| **Settings** | remembered between runs, so the modem comes back where it was left |
| **Files** | ZMODEM send and receive |
| **Network** | PPP (RFC 1661/1662) with LCP and IPCP, and a ping over it |
| **Internet** | our own TCP (RFC 9293) and a SOCKS 5 proxy: a browser on one machine, the internet on the other |
| **Line** | full-duplex sound card, or a WAV to replay |

Two of these on a call carry the internet. The Network panel brings up PPP:
the end that answered hands out an address, the end that dialled asks for one
and is told, and an ICMP echo crosses and comes back with a round trip on it.
Tick *carry web traffic* and the end that answered offers its connection —
point a browser on the dialling machine at `socks5://127.0.0.1:1080` and it
goes out through the modem.

TCP is ours, written against RFC 9293: the eleven states, retransmission with
RFC 6298's estimator, Nagle, delayed acknowledgements, zero-window probing,
out-of-order reassembly, and RFC 5681's fast retransmit. Nothing in the path
belongs to the operating system except the socket at the far end that actually
reaches the internet — no driver, no adapter, no route, no administrator.

The tallest test does the whole of it against a simulated line: V.8, V.22bis,
V.42, V.42bis, PPP, IPCP, TCP, SOCKS 5, and a real web server on the loopback.
Connected six seconds into the call, network phase at seven, a page back at
2400 bit/s — and it runs without a sound card.

The window around it is a scope: waterfall, spectrum, constellation, LED
faceplate, decoded transcript, and a log of every frame both ends sent. What
the echo canceller is taking out, and where on the line it found the
reflection, are on the panel too — on a two-wire pair that decides everything
and is otherwise invisible, since a constellation full of noise looks the same
whether the noise is the line or this modem listening to itself.

Portable in principle — `cpal` for the audio, `eframe` for the window, no
OS-specific code — but only built and run on Windows so far.

## Work in progress

- **V.32 at 9600** connects to a real modem, agrees on the trellis code and
  passes data, but not reliably. The receiver reaches 30 dB about half a
  second after connecting and then loses the carrier: the error while it goes
  is all across the radius and none along it, and the equaliser — frozen at
  the peak to check — leaves the radial part untouched. So it is the carrier
  loop, not the line and not the equaliser. Two calls now say the same thing.
  The rates above it are more crowded still and will want the same fix.
- **14 400 works on one virtual cable**, both ends, with V.42 and V.42bis on
  top and 0.76 of everything each modem says coming back a sixth of a second
  later. Over a VoIP trunk to a real modem it has not been reached.
- **The rate signal gets misread, and it costs the rate.** One recorded call
  to a real V.32bis modem: the far end offered 4800 through 14 400 and sent
  that same sixteen bits 201 times; this end read one corrupted copy of it,
  answered with an E calling for 4800, and connected there. 5.3.1 asks for two
  consecutive identical sequences and two is what a systematic misread
  produces — counted at the locked phase, the true sequence ran 53 consecutive
  and every wrong one ran once, except the one that was acted on, which ran
  twice. Waiting for a third was tried and is worse: on a line returning the
  transmitter at unity a correct reading never happens three times running, so
  a modem that holds out never connects. The reading wants a better receiver
  under it, not a stricter test above it.
  [tools/read_rate_signals.py](tools/read_rate_signals.py) reads them off a
  recording.
- **The round trip is a second and a quarter, and V.32's start-up has no room
  for it.** MicroSIP to a real modem measures 2947 symbols there and back. Two
  faults that only show up at that length have been found and fixed. 5.4.1's NT
  and 5.4.2's MT are counter readings the two ends compare, and this end was
  handing the pre-roll a trimmed version of one: the far end heard the S, ceased
  transmitting as 5.4.2 tells it to, looked again MT later and found the S had
  ended 30 ms earlier. It waited five seconds for another and then started the
  call from the answer tone, twice. And 5.4.1's "transmission of R2 shall
  continue until an incoming rate signal R3 is detected" was read as written,
  so this end talked at a modem that had gone back to the beginning for
  twenty-three seconds and let it hang up. Both are guarded by tests now;
  neither has been tried on the line again yet.
- **Two of these on one virtual cable** fail in one direction only. The
  answering end reads a clean thirty-two point constellation and brings up
  V.42 and V.42bis; the originating end sees noise. Measured off a recording,
  the originating end's own signal comes back at 121 ms and is 0.57 correlated
  with everything it hears, which is the asymmetry: only the calling modem has
  to receive the far end's second training segment while transmitting.

## Next

The fax data pumps — V.27ter, V.29, V.33 and V.17 — which share most of their
machinery with what is already here: V.33 and V.17 are the same trellis code
again, on a leased line and a fax call respectively. Then V.34 at 33 600.

Alongside them: PAP and CHAP, so something other than another BinModem can
dial in; and MNP as an alternative to LAPM, since it is what a modem without
V.42 will offer.

## Running it

```bash
cargo run -p gui --release -- --live
```

Or `dist.bat`, which leaves a single `dist\binmodem.exe` holding the scope, the
terminal, an answering board and a capture to replay, with nothing to install.
[docs/usage.md](docs/usage.md) has the rest — the AT interface, the live-line
setup, ZMODEM and the telnet terminal.

## Licence

GPL-3.0-or-later. The ITU-T Recommendations themselves are not redistributed
here; `tools/fetch_specs.sh` downloads them.
