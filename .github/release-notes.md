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

New since v0.3.0, all of it from recordings of calls to a real modem over a
trunk with a second and a quarter of delay in it — which is where V.32's
half-duplex start-up stops forgiving anything.

- **14 400 reached with a real modem**, and then found unreadable, which is
  the next thing to fix. Both ends offered every rate and agreed on the top
  one.
- **A rate that cannot be read is given up.** 7 begins a retrain on
  "unsatisfactory signal reception" and leaves the definition open; ours was a
  distance, which is a quarter of the gap between points at 4800 and one and a
  half gaps at 14 400 — further than a symbol can land from the nearest point,
  so at the rates it mattered for it could not fire at all. It is a fraction of
  the gap now, and the retrain that follows offers less than it did. On the
  line: 14 400 came up, could not be read, and the far end answered the reduced
  offer with 12 000. The first rate this modem has renegotiated with anything
  but itself.
- **Three start-up faults that only appear on a slow line.** The period one end
  holds a signal up for and the period the other waits before looking for it
  are counter readings meant to be compared, and one of them was being trimmed;
  the wait for the far end's final rate signal had no end at all, so the modem
  talked at a modem that had gone back to the beginning for twenty-three
  seconds; and a single phase reversal was once taken for two, giving a round
  trip of 53 ms on a line whose real one is 1.2 seconds.
- **Silence during a retrain is no longer read as a hangup.** The training
  segment is the one stretch the far end must be quiet for, and on a line that
  reflects almost nothing that is indistinguishable from a far end that has
  gone. The modem was ending calls in the middle of retrains it had asked for.
- **The panel says what the receiver is managing**, as a fraction of the
  distance between the points it is choosing between. A tenth is a clean lock,
  a quarter is where the modem gives up on the rate, a half is a coin flip.

V.32 at 9600 connects to a real modem and passes data, but loses the carrier
about half a second in. `AT+MS=V32B,1,4800,14400` is what has been used for the
calls that stayed up.

Checksums are in `SHA256SUMS.txt`.
