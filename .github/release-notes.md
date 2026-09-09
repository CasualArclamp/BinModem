One file. Download `binmodem.exe` and run it — nothing to install, and no
Visual C++ redistributable, because the C runtime is linked in. The build is
checked for that before it is published.

```
binmodem.exe                  a modem on a real line
binmodem.exe --devices        what audio this machine has
binmodem.exe --telnet         a board over a socket
binmodem.exe --capture        replay the golden capture
```

The window opens on a real line, because that is what the program is for.
Reaching a line outside the machine wants two virtual audio cables, so the
output can go to a softphone's microphone while the input comes from its
speaker — [docs/usage.md](https://github.com/CasualArclamp/BinModem/blob/main/docs/usage.md)
has the setup. `--capture` replays a real Bell 103 call instead and needs no
audio at all, which is the quickest way to see whether it runs.

Bell 103, V.22, V.22bis, V.32 and V.32bis; V.8 negotiation, V.42 error control
and V.42bis compression; a V.250 AT interface, an ANSI/CP437 terminal and
ZMODEM.

New since v0.2.0:

- **V.32bis to 14 400.** One trellis code serves 7200, 9600, 12 000 and
  14 400, and only the number of bits riding through it untouched changes.
  Every constellation was read off the figures by position.
- **V.32 and V.32bis are separate carriers**, because V.250 gives them
  separate names: `AT+MS=V32` stops at 9600 and `AT+MS=V32B` goes to 14 400.
- **The retrain of V.32bis 7.** A far end that gives up and starts again is
  followed now, from either end of the call, instead of being talked over.
- **The internet over a call.** PPP with LCP and IPCP, a TCP written against
  RFC 9293, and a SOCKS 5 proxy: tick *carry web traffic* on the end that
  answered and point a browser on the other machine at
  `socks5://127.0.0.1:1080`.

V.32 at 9600 connects to a real modem and passes data, but loses the carrier
about half a second in; `AT+MS=V32B,1,4800,14400` is what has been used for
the calls that stayed up.

Checksums are in `SHA256SUMS.txt`.
