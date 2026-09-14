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
and V.42bis compression; group 3 fax over V.29 and V.27 ter; a V.250 AT
interface, an ANSI/CP437 terminal and ZMODEM.

New since v0.4.0: **it is a fax machine.** Everything here was tried against a
public fax service over a VoIP trunk, and most of what was fixed was found
that way.

- **Send a picture, or wait for one.** The Fax window loads an image and turns
  it into a page — 1728 pels across, at standard or fine resolution,
  thresholded or dithered — and shows what it will cost at each rate before
  anything is dialled. A page sent to the fax service is accepted and
  confirmed; a page sent back from it arrives, is drawn in the window, and
  saves as a PNG with the pels made square.
- **T.30 from either end**: the calling and called tones, identification and
  capabilities, the training check, the page, the receipt and the disconnect,
  with retries, a rung down the rate ladder for every refused training check,
  and a disconnect rather than silence when a call is given up. The far end's
  number and everything its DIS says appear as soon as it says them.
- **V.29 at 9600 and 7200, V.27 ter at 4800 and 2400.** Every point of both
  constellations and all of their training sequences were read off the figures
  in the PDFs, since the extracted text turns the square root of two into "2".
  Table 4 of V.27 ter prints its training pattern only at the two ends; the one
  scrambler seed that reproduces both is the register load its appendix states.
  The Offer boxes decide which the call may use.
- **A real machine's training check is not refused any more.** It puts a fifth
  of a second of plain carrier and twenty milliseconds of silence in front of
  its training, and that silence was being taken for the end of the burst. The
  receiver had read the check itself as 7195 zeros out of 7200.
- **A carrier is found wherever it starts.** No test had ever made a receiver
  look for one, because every loopback started both oscillators in phase. With
  the seven hertz V.27 ter and V.29 allow for and any timing skew, at full
  level and thirty decibels down, V.29 failed 45 of 80 cases and V.27 ter at
  2400 failed 40 of 200. Both now measure the carrier from their training
  before trusting their own decisions, and pass all of them.
- **The scope follows the fax**: an eye for the 300 bit/s frames and a
  constellation for the page, the one going out while this end sends.
- **V.42 survives a retrain during protocol establishment**, instead of
  spending its attempts at a link on a line that was being rebuilt and falling
  back to start-stop characters. The panel also says when this end is too
  quiet for the far end, not only too loud.
- **A crash leaves a line behind** in `crashes.txt` beside the program, naming
  where it happened. Please send it if one appears.

Fax is one page per call, with no error correction mode, and V.17 at 14 400 is
not written yet. The modem stays in fax class after a fax call; `AT+FCLASS=0`
makes it a modem again.

V.32 at 9600 still connects to a real modem and loses the carrier about half a
second in. `AT+MS=V32B,1,4800,14400` is what has been used for the calls that
stayed up.

Checksums are in `SHA256SUMS.txt`.
