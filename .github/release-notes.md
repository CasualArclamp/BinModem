Download `binmodem.exe` and run it; `dialsnoop.exe` beside it is the call reader on its own. There is nothing to install, and no Visual C++ redistributable is needed because the C runtime is linked in.

```
binmodem.exe                  a modem on a real line
binmodem.exe --devices        what audio this machine has
binmodem.exe --telnet         a board over a socket
binmodem.exe --capture        replay the golden capture
binmodem.exe --review         read a recording of a call
dialsnoop.exe                 the same reader, as a program of its own
```

[docs/usage.md](https://github.com/CasualArclamp/BinModem/blob/main/docs/usage.md) has the setup.

## What's new in 5.9.0

### V.32 and V.34: the far echo on a SIP trunk (PR 106, PR 110)

- On a SIP trunk the only echo is the far hybrid, about 1 s back. The window showed "reflection not found", and the canceller took out 0 dB.
- The echo search now starts one round trip after TRN starts. On a long line, TRN is longer, so that the far taps have time to learn.
- The canceller keeps a round trip of what went out, and it follows the drift of the far clock.
- V.32 also follows a far echo that goes away in double talk.
- **Tested** live to NIST's ACTS: the V.32 canceller took out 26.7 dB, and V.32 4800 with V.42 held. V.32bis 9600 to ACTS still does not hold.

### V.32bis: rate renegotiation (PR 108, PR 112)

- Before, a bad line caused a full retrain, which takes about 12 s on a 1 s round trip.
- BinModem now does the rate renegotiation of V.32bis clause 8, both ways and in both roles. A step down takes less than 1 s.
- 1 s of bad reception between two V.32bis modems now starts a step down. If the far end does not answer, the modem does a full retrain.
- **Not tested** live. New tests run two BinModem ends over a simulated SIP line.

### Call card

- Each QAM bar has a tick at its margin point. The bar is red below the tick, amber up to 3 dB above it, and green above that (PR 105).
- On V.34, the margin point allows 6.02 dB for the trellis code (PR 107).

### Terminal: send menu and character format (PR 109, PR 111)

- Ctrl+right-click on the terminal opens a send menu: Break, Hard break, the control characters, and Telnet commands.
- A break goes as a V.42 BRK on an error-corrected call, and as `IAC BRK` on Telnet.
- A button after the font slider opens a panel with Data bits, Stop bits, Parity and Flow control.
- **Not tested** on a real call.
