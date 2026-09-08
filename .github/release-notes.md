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

Bell 103, V.22, V.22bis and V.32 4800; V.8 negotiation, V.42 error control and
V.42bis compression; a V.250 AT interface, an ANSI/CP437 terminal and ZMODEM.
V.32 at 9600 connects and then delivers noise — we implement the uncoded
variant and real modems send the trellis-coded one — so `AT+MS=V32,1,4800,4800`
is the setting for that one until it is done.

Checksums are in `SHA256SUMS.txt`.
