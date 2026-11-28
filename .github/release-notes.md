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

## What's new in 5.8.0

### Dark and light theme (issue 100)

- On a light Windows theme, some text in the window was light grey on a white panel.
- A key at the right end of the tab strip now sets the theme. ◑ follows Windows (the default), 🌙 is dark and ☀ is light. The window keeps the choice.
- In the light theme, all text on the panels has dark colours with the same hues, with a contrast of 4.5:1 or more. The faceplate, the readout, the scopes and the terminal stay dark.
- DialSnoop is always dark.

### Automode: far ends with no V.8, such as NIST's time service

- NIST's ACTS answers with plain ANS, then V.22bis ones, then V.32 AC. With Auto on, a call to it did not connect.
- Under automode, the V.32 caller now sends AA only after AC, or after 2 s of quiet after ANS. Plain V.32 keeps V.32 5.4.1.
- Over its own AA, a tone must hold 1.5 s before the call leaves V.32.
- V.22bis now measures the round trip, and V.42 adds it to T400 and T401. On a long line, error control no longer falls away.
- **Tested** live to ACTS: V.22bis 2400 with V.42 and compression, on two calls. V.32bis to ACTS still loses to the echo of our own data. That needs a separate fix.

### HST: channel reversals (issue 84)

- **No stale rate request.** While BinModem waited to take the forward channel, its fallback could ask for a lower rate from the far carrier as it stopped. That request went out after the handover, and the far end then waited for a training that did not come. The fallback now waits until the handover is done.
- **0x2R, retrain at a rate.** BinModem now acts on it from both ends. It also asks for one when it is at the slowest rate and has lost the line.
- **Not tested** against libv90modem or a real Courier. New tests run two BinModem ends over a simulated line.
