One file. Download `binmodem.exe` and run it. There is nothing to install, and no Visual C++ redistributable is needed because the C runtime is linked in.

```
binmodem.exe                  a modem on a real line
binmodem.exe --devices        what audio this machine has
binmodem.exe --telnet         a board over a socket
binmodem.exe --capture        replay the golden capture
```

[docs/usage.md](https://github.com/CasualArclamp/BinModem/blob/main/docs/usage.md) has the setup.

## What's new in 2.0.0

### Super G3: fax at 33 600

- **V.34's half-duplex mode, for fax** -- T.30 Annex F on V.34 clause 12. V.8 goes in front of the call: the answering end sends ANSam and offers V.34 half-duplex in its joint menu, and the calling end hears ANSam and sends its call menu. Then phase 2 probes the line one way, phase 3 trains the page channel, and a 1200 bit/s control channel runs both ways at once, carrying every T.30 frame. Each page goes over the primary channel at up to 33 600, retrained on its own PP in front of every burst. There is no training check and no failure to train: the rate comes from the control channel's MPh exchange.
- **A real machine's page came in on it.** A Super G3 fax sent BinModem a page at 33 600 bit/s on 3429 baud, in JBIG under error correction, with the page channel trained to 53 dB.
- **A misprint in V.34, settled by that machine.** Equation 10-2 prints PPh, the control channel's training sequence, with a digit one where the clause defines a variable I and then never uses it. Read as printed it is a square wave on one diagonal; read with I it is a perfect periodic sequence, like the duplex PP beside it. It was built the second way, and a real machine's PPh is exactly that.
- **A V.34 box** in the fax window's offer row, on by default. Unticked, a call is what it was.

### JBIG

- **T.85's profile of T.82**, single-progression sequential, sending and receiving. It matches T.82's own test data to the byte: the 25-byte arithmetic coder sequence, and every byte count of the three artificial-image tests, with Annex C's adaptive template making the same move at the same stripe. Four misprints in T.82's figures and tables turned up on the way, and each is settled by a test.
- **Offered and used under error correction** when both ends have it (DIS and DCS bits 78 and 79), ahead of MMR. The real machine's page above came in JBIG. A tick box beside ECM turns it off.

### What the page is going in, where it can be seen

- **While a page moves,** the fax window shows its coding, its modulation and rates, and error correction as coloured badges: green for the best a fax has -- JBIG, V.34, ECM -- then yellow and orange for the rungs below, and *best mode* when all three are green.
- **The page to send** lists its size in all four codings and its time at V.34's 33 600 beside the other rates.

### A V.34 fax's call menu is answered

- **A caller that took T.30's plain answer tone for ANSam** sent V.8 call menus for as long as the call lasted, straight past every DIS. A recording of one is in the tests now. The answering end hears the call menu and answers it with a joint menu, then goes on with T.30.

### Also

- **The symbols scope draws a V.34 page to scale.** Its range was taken in the decoder's grid units, 41 at 33 600, against points of about one and a half, and every point of the first real page was drawn as one dot at the centre.
- **The far-end panel** no longer reads a V.34 caller's DCS as V.27 ter. T.30 sends its rate bits as zero on V.34 (Table 2, Note 33), and the panel now says V.34 and the page's rate and symbol rate.
- **The sending end of a Super G3 call waits for the far end's E** before it sends its own. See the first status item below for why.

## Status

- **Sending over Super G3 is not yet confirmed against a real machine.** On the first try, both control channels came up, and each end read the other's MPh and E. Then the far end sent its NSF and DIS and never answered our TSI and DCS, four times over. Every frame in the recording decodes whole in both directions, with the headers right. The likeliest cause is that our E reached that machine about 50 ms before it had sent its own, so it never started reading ours. The last change above answers that, but it has not been tried live yet. Untick V.34 to send at V.17.
- **V.32's round-trip measurement can still be halved by a lost packet.** On a V.32bis call over a trunk, the concealment brought the far end's alternation back inverted, and the reversal that came out of it stopped the clock at 706 ms on a line whose real round trip is 1.22 s. The call connects regardless; the bad reading still costs the echo canceller's tap placement.
- **One ISP's V.90 pool sometimes stops dead** just as its modem takes up our constellation, at the end of phase 4 or in a renegotiation. Every CP we sent was checked against V.90 Table 14 bit for bit, and they are correct.
- **Known:** after a V.90 call falls back to V.34, the V.34 retrain watch can still take the hang-up beep for tone B.
- **Compression between two BinModems in V.90** is not yet offered correctly.
- **V.92:** planned and in progress on a branch; not in this release.
- **Not yet tried against real far ends:** V.17, and the V.29 and V.27 ter receivers, have been measured against our own modems and simulated lines only.
- **Unchanged:** the fax window sends one page per call.

2175 tests. Checksums are in `SHA256SUMS.txt`.
