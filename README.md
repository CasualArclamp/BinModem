# BinModem

<img width="1319" height="1007" alt="dialupmodem2_0IgsyArLo3" src="https://github.com/user-attachments/assets/99c3c5a9-0a1f-4c6d-af22-05f37344e7d3" />

A dial-up softmodem in Rust. Everything a modem does — the tones, the
handshakes, the error control, the compression — is code, and the line is a
sound card. No DSP chip, no driver blob, and none of a winmodem's dependence on
one vendor's Windows.

Written against the ITU-T Recommendations, with clause numbers cited in the
source for every normative constant. 864 tests.

## What works

It places and answers real calls. Full interactive sessions — public dial-up
gateways, reached over a VoIP trunk — have crossed it at V.22bis 2400 and at
V.32 4800, with V.42 error control and V.42bis compression, byte for byte
correct including the ANSI. Where a far end answers no XID at all and simply
announces compression in band, that is followed too, and 1957 octets of one
board's screen are kept as a test vector.

At 9600 it negotiates V.32's trellis code and carries data with it. Every
number of that code — the differential table, the thirty-two point
constellation, the convolutional encoder — was read off the Recommendation's
own figures rather than out of extracted text, which loses the sign of every
coordinate, and checked three ways: against the magnitudes the table does
survive with, against the set partition the code exists to have, and against a
mean power that had to come out at ten.

| | |
|---|---|
| **Modulations** | Bell 103 (300), V.22 (1200), V.22bis (1200/2400), V.32 (4800/9600, both codings) |
| **Negotiation** | V.8 CM/JM/CI/CJ and ANSam; V.25 answer tone told apart from it |
| **Error control** | V.42 LAPM — detection, HDLC, XID, REJ/SREJ, mod-128 |
| **Compression** | V.42bis, negotiated in XID or followed in band |
| **Commands** | V.250 AT: `+MS`, `+ES`, `+DS`, `+ER`, `+DR`, S-registers, `+++` |
| **Terminal** | ANSI/CP437 with mouse reporting; telnet (RFC 854) to use it alone |
| **Settings** | remembered between runs, so the modem comes back where it was left |
| **Files** | ZMODEM send and receive |
| **Network** | PPP (RFC 1661/1662) with LCP and IPCP, and a ping over it |
| **Line** | full-duplex sound card, or a WAV to replay |

Two of these on a call can carry IP. The Network panel brings up PPP over
the connection: the end that answered hands out an address, the end that
dialled asks for one and is told, and an ICMP echo crosses and comes back with
a round trip on it. A test does the whole of that against a simulated line —
V.8, V.32 at 9600, V.42, V.42bis, PPP, IPCP, ping — so it can be checked
without a sound card.

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
- **Two of these on one virtual cable** fail in one direction only. The
  answering end reads a clean thirty-two point constellation and brings up
  V.42 and V.42bis; the originating end sees noise. Measured off a recording,
  the originating end's own signal comes back at 121 ms and is 0.57 correlated
  with everything it hears, which is the asymmetry: only the calling modem has
  to receive the far end's second training segment while transmitting.

## Next

Getting real traffic over the PPP link: authentication (PAP and CHAP), a
userspace TCP stack, and a SOCKS proxy, so a browser on one machine can reach
the internet through a modem call to another — with no driver and no
administrator.

Alongside it: V.32bis (14 400) and V.34 (33 600), which the trellis code is
most of the groundwork for, and MNP as an alternative to LAPM, since it is
what a modem without V.42 will offer.

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
