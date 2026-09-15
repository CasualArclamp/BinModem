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

Bell 103, V.22, V.22bis, V.32, V.32bis and V.34; V.8 negotiation, V.42 error
control and V.42bis compression; group 3 fax over V.29 and V.27 ter, with error
correction mode and MMR; a V.250 AT interface, an ANSI/CP437 terminal and
ZMODEM.

New since v0.5.0: **33 600.** V.34 was built against a public dial-up service
over a VoIP trunk, one live call at a time, and the call that worked came up at
31 200 towards this end and 33 600 from it, brought V.42 up over the top, and
logged in and carried a session onward for a minute.

- **V.34, all of it**, behind `AT+MS=V34` or 33600 in the rate row. Phase 2
  swaps capabilities, measures the round trip and probes the line in both
  directions; phases 3 and 4 train both receivers and settle the rates in MP
  sequences; data mode is superframes, shell mapping, the 16, 32 and 64-state
  four-dimensional trellis codes, precoding and non-linear encoding. The
  start-up was checked against a Conexant recording and the live calls, and
  data mode against the far modem's own B1, read as the ones it is. The
  constellations and trellis encoders were read off the figures in the PDF, and
  the framing checked against its tables as printed, rather than trusting the
  extracted text.
- **Through a VoIP jitter buffer.** A softphone makes up or drops twenty
  milliseconds of audio every few seconds. The start-up follows the jumps, and
  data mode now lives through them too: the receiver re-times itself, and finds
  its place in the frames again from the bit inversions every superframe
  carries, in about a third of a second, while V.42 sends again what was lost.
  One call had a slip swallow the far modem's E, the signal data mode starts
  on; that call is rescued the same way instead of waiting for the E until it
  gives up.
- **Rate renegotiation and cleardown** from either end. The far modem asked,
  six seconds into its first data call, to be sent 28 800 rather than 33 600;
  this end now answers at once, the call stays up through it, and the panel
  says what the new rates came to.
- **A far end that talks first.** Its login banner goes to the terminal, and a
  FidoNet mailer's `**EMSI_REQ` is no longer read as two V.42 answer patterns
  that turn error control off.
- **Data mode at the power of training**, as 10.1.3 asks. Low mapping frames
  use the cheaper half of the shell mapper's combinations, and counting them as
  high had left 33 600 going out 7.5% short.
- **The scope draws data mode**: its hundreds of points, scaled to reach, with
  enough symbols kept to land on all of them. Click the scope to open it large.
  `tools/plot_constellation.py` draws any stretch of a recorded call the same
  way, side by side.
- **Fax error correction mode.** T.30 Annex A between two of these: numbered
  frames and a request for any the far end could not read, so noise on a page
  costs a few frames sent again instead of streaks. Under it the page can go
  in MMR, T.6's coding, at under half the size of Modified Huffman, and
  Modified READ is there too. A page arriving is drawn a row at a time.
- **The ZMODEM window** has folder buttons for the file to send and the folder
  to receive into, and a speed worth reading: the last few seconds' and the
  whole file's, the time left and the share of the line the file is getting,
  counted from when the file starts moving rather than from the handshake. What
  a transfer came to stays on the window when it is over.
- **The line is driven at -20 dB by default**, the level the calls through a
  softphone were placed at, which leaves the shaped constellations' peaks well
  under full scale.

V.34 does not yet follow a full retrain, which a far end falls back to when a
renegotiation goes unanswered or the line changes too much for one; the call
ends there instead. 33 600 wants about 35 dB of signal to noise and a VoIP line
gives 33 or 34, so a far end asking for 31 200 or 28 800 is the line, not the
modem.

Fax is one page per call, and V.17 at 14 400 is not written yet. The modem
stays in fax class after a fax call; `AT+FCLASS=0` makes it a modem again.

V.32 at 9600 still connects to a real modem and loses the carrier about half a
second in. `AT+MS=V32B,1,4800,14400` is what has been used for the calls that
stayed up.

Checksums are in `SHA256SUMS.txt`.
