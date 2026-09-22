One file. Download `binmodem.exe` and run it. There is nothing to install, and no Visual C++ redistributable is needed because the C runtime is linked in.

```
binmodem.exe                  a modem on a real line
binmodem.exe --devices        what audio this machine has
binmodem.exe --telnet         a board over a socket
binmodem.exe --capture        replay the golden capture
```

[docs/usage.md](https://github.com/CasualArclamp/BinModem/blob/main/docs/usage.md) has the setup.

## What's new in 1.2.1

### V.90: the softphone's hang-up beep is no longer taken for tone B

- **Tone B must be as loud as the first one:** the first phase 2 of a call records how loud the server's tone B came. From then on, both the data-mode retrain watch and a retrain's own phase 2 ignore anything under half that level. The 1200 Hz beep a softphone plays when the far end hangs up arrives 10 dB quieter, and no longer sends a dead call back into phase 2. A real retrain from the server is still caught at the same moment.
- **A retrain's phase 2 waits for tone B:** it used to take the server's own data for tone B and reverse tone A early (9.2.2.1.3), then sit out a two-second recovery. Retrains are now one to two seconds shorter, and calls come out of a storm of audio holes more often.

### V.8: an answering tone is held before it is believed

- **Silence is not a tone:** the detector took a step into silence for an answer tone, and a tone that had stopped went on reading as present for about two seconds. A tone that stops now reads as gone within about 15 ms.
- **No decisions on one instant:** a plain V.25 tone must be held for Te and ANSam for a quarter of a second before the calling modem acts. On a live V.32 call the modem gave up on V.8 two seconds before the far end's ANSam began; across the saved captures, about 23 such false early decisions are gone.

### V.42bis and V.44: a stream that will not decode starts the link again

- A compressed codeword that would not decode used to end the call. The link is now re-established, which starts both dictionaries again (V.42bis 5.8, V.44 7.15), up to three times before it is released. The transcript says which compression failed and what was done.

### V.90 transcript

- The MP line now says everything the server asks of our transmitter: trellis, non-linear encoding, shaping and precoding coefficients.

## Status

- **One ISP's V.90 pool sometimes stops dead** just as its modem takes up our constellation, at the end of phase 4 or in a renegotiation. Every CP we sent was checked against V.90 Table 14 bit for bit, and they are correct. The same server has also reached data mode at 54 666 and 56 000.
- **Known:** after a V.90 call falls back to V.34, the V.34 retrain watch can still take the hang-up beep for tone B.
- **Compression between two BinModems in V.90** is not yet offered correctly.
- **V.92:** planned and in progress on a branch; not in this release.
- **Unchanged:** the fax window sends one page per call, and V.17 is not written yet.

1643 tests. Checksums are in `SHA256SUMS.txt`.
