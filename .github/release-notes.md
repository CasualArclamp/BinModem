One file. Download `binmodem.exe` and run it. There is nothing to install, and no Visual C++ redistributable is needed because the C runtime is linked in.

```
binmodem.exe                  a modem on a real line
binmodem.exe --devices        what audio this machine has
binmodem.exe --telnet         a board over a socket
binmodem.exe --capture        replay the golden capture
```

[docs/usage.md](https://github.com/CasualArclamp/BinModem/blob/main/docs/usage.md) has the setup.

## What's new in 1.3.0

### V.32 and V.32bis: a new receiver

- **Built on the V.34 receiver's design:** it keeps the stored samples, uses a half-symbol equaliser solved by least squares on the far end's known TRN, and holds one gate over three loops. The old receiver trained blind and never measured the carrier, so on the dense constellations it pulled in only 10–15°.
- **The carrier is measured from S before anything tracks it:** ±7 Hz (V.32 2.1) and ±200 ppm are held at every rate. Before, 9600 and above failed at their working SNR, and at −7 Hz the calling modem never trained.
- **Slips are followed, not retrained:** one dropped or repeated sample used to force a retrain, which then gave up 12 000 and 14 400 for the rest of the call. The receiver now rewinds and reads the samples again. A 20 ms concealment in the far end's S no longer stops the start-up.
- **The echo canceller follows drift:** over a sound-card loopback with 5 to 100 ppm between the two directions, 14 400 now holds with no retrains. Before, it fell to 4800 or never connected.
- **Our transmitter follows the Recommendation:** TRN from symbol 256 on is Table 5 as printed; two dibits had been swapped. The four-point slicer's boundaries lie halfway between the states. 12 000 and 14 400 go out at the level the figures draw them.
- Every acceptance line the rebuild was measured against failed before and passes now: working SNR, carrier and clock offset, slips, drift, a model of a VoIP line, and a real loss that must retrain and come back at 14 400.

### Fax: V.17, and V.29 and V.27 ter rebuilt

- **V.17 at 14 400, 12 000, 9600 and 7200:** the transmitter, the receiver and the call. Training points A to D were read from the rendered figures, and sit at the same places at every rate. The Fax window offers V.17 now, and it can be unticked.
- **V.29 and V.27 ter receivers on the same core:**
  - The carrier detector no longer latches on line noise.
  - A burst after a long quiet line is found, and a talker-echo tone no longer raises a phantom burst.
  - Every arrival phase and ±7 Hz are held.
  - Whole calls stay at 9600 and 4800 over hiss, where they used to fall back.

## Status

- **One ISP's V.90 pool sometimes stops dead** just as its modem takes up our constellation, at the end of phase 4 or in a renegotiation. Every CP we sent was checked against V.90 Table 14 bit for bit, and they are correct. The same server has also reached data mode at 54 666 and 56 000.
- **Known:** after a V.90 call falls back to V.34, the V.34 retrain watch can still take the hang-up beep for tone B.
- **Compression between two BinModems in V.90** is not yet offered correctly.
- **V.92:** planned and in progress on a branch; not in this release.
- **Not yet tried against real far ends:** V.17, and the new V.32bis, V.29 and V.27 ter receivers, have been measured against our own modems and simulated lines only.
- **Unchanged:** the fax window sends one page per call.

1761 tests. Checksums are in `SHA256SUMS.txt`.
