# BinModem

<img width="1319" height="1007" alt="dialupmodem2_0IgsyArLo3" src="https://github.com/user-attachments/assets/99c3c5a9-0a1f-4c6d-af22-05f37344e7d3" />

A dial-up softmodem in Rust. Everything a modem does — the tones, the
handshakes, the error control, the compression — is code, and the line is a
sound card. No DSP chip, no driver blob, and none of a winmodem's dependence on
one vendor's Windows.

Written against the ITU-T Recommendations, with clause numbers cited in the
source for every normative constant. 768 tests.

## What works

It places and answers real calls. A full interactive session — a public dial-up
gateway, reached over a VoIP trunk — has crossed it at V.22bis 2400 with V.42
error control and V.42bis compression, byte for byte correct. Where a far end
answers no XID at all and simply announces compression in band, that is
followed too, and 1957 octets of one board's screen are kept as a test vector.

| | |
|---|---|
| **Modulations** | Bell 103 (300), V.22 (1200), V.22bis (1200/2400), V.32 (4800/9600) |
| **Negotiation** | V.8 CM/JM/CI/CJ and ANSam; V.25 answer tone told apart from it |
| **Error control** | V.42 LAPM — detection, HDLC, XID, REJ/SREJ, mod-128 |
| **Compression** | V.42bis, negotiated in XID or followed in band |
| **Commands** | V.250 AT: `+MS`, `+ES`, `+DS`, `+ER`, `+DR`, S-registers, `+++` |
| **Terminal** | ANSI/CP437 with mouse reporting; telnet (RFC 854) to use it alone |
| **Files** | ZMODEM send and receive |
| **Line** | full-duplex sound card, or a WAV to replay |

The window around it is a scope: waterfall, spectrum, constellation, LED
faceplate, decoded transcript, and a log of every frame both ends sent.

Portable in principle — `cpal` for the audio, `eframe` for the window, no
OS-specific code — but only built and run on Windows so far.

## Work in progress

- **V.32 at 9600** connects and then delivers noise. We implement the uncoded
  variant; real modems send the trellis-coded one. `AT+MS=V32,1,4800,4800`
  works around it.
- **V.32 over VoIP** needs echo cancellation turned off on the trunk. Both
  directions share the band and a canceller in the middle cannot separate them.
  V.22bis is unaffected, because it splits the band instead.

## Next

V.32 trellis coding, then V.32bis (14 400) and V.34 (33 600). After that, the
part that makes it a modem to the rest of the machine rather than only to its
own window: a COM port, so Windows Dial-Up Networking can dial it.

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
