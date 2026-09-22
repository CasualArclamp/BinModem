One file. Download `binmodem.exe` and run it. There is nothing to install, and no Visual C++ redistributable is needed because the C runtime is linked in.

```
binmodem.exe                  a modem on a real line
binmodem.exe --devices        what audio this machine has
binmodem.exe --telnet         a board over a socket
binmodem.exe --capture        replay the golden capture
```

[docs/usage.md](https://github.com/CasualArclamp/BinModem/blob/main/docs/usage.md) has the setup.

## What's new in 1.2

### V.90 reaches 56k data on live calls

- **A rate menu:** the status panel has a V.90 rate drop-down listing every downstream rate. Once a call has read its DIL, green rates are predicted to read cleanly and red ones are predicted to make errors. Any of them can be tried. During a data call, a choice renegotiates to that rate at once (V.90 9.6). Before a call, it pins the rate the next start-ups ask for; "auto" leaves the choice to the modem.
- **Falls back when the line is disturbed:** data mode counts its own wrong decisions, remembers them for fifteen seconds, and renegotiates to a slower rate when disturbances keep coming. A fall back drops at most eight bits a frame. Packet loss concealment, jitter-buffer slips and audio holes from a softphone are recognised and not held against the line. The transcript says why the rate watch acted, and on what numbers.
- **Room in the first rate:** the rate chosen at the end of the DIL leaves a margin, rather than every level sitting exactly at the design spacing.
- **Spectral shaping:** where the path cuts the top of the band, as a VoIP provider's did, the modem asks the server to shape its spectrum (5.4.5) and comes up faster than it would unshaped.
- **Phase 4 in the transcript:** every CP sent and MP found, E, Ed and B1d get a line each, also written beside the recording.
- **Dead lines:** silence is no longer taken for the server's Ed. A server that stops during phase 4 ends the phase within about a second.
- **CP:** every CP now sends one constellation field per data frame interval.
- **Our V.90 server:** its look-ahead sends every frame where it belongs, so a caller asking for shaping gets it.

### V.42 and V.8, from a live call

- **XID fixed four ways:** the poll bit, option bit 16, one command with a retransmit timer instead of a burst, and a V.44 offer a far end can read. A real server has now answered our XID, and V.44 has been negotiated live.
- **LAPM and HDLC:** an I or supervisory frame stands in for a lost UA, an abort keeps the frame's trailing ones, and XID reads option masks of any length.
- **V.8:** a sequence the jitter buffer tore is dropped alone, not along with the whole one after it. Two menus count as identical only when their sequences are.
- **V.34 phase 2:** a calling modem whose ranging reversal went unanswered now sends tone B again. It used to fall silent, and both ends waited until phase 2 timed out.

### In the window

- **Constellations:** every modulation's constellation is drawn in the same style.

## Status

- **One ISP's V.90 pool sometimes stops dead** just as its modem takes up our constellation, at the end of phase 4 or in a renegotiation. Every CP we sent was checked against V.90 Table 14 bit for bit, and they are correct. The same server has also reached data mode at 54 666 and 56 000.
- **Known:** the softphone's end-of-call beep can be taken for the server's tone B, so a call that has already ended goes back to phase 2 briefly.
- **Compression between two BinModems in V.90** is not yet offered correctly.
- **V.92:** planned and in progress on a branch; not in this release.
- **Unchanged:** the fax window sends one page per call, and V.17 is not written yet.

1624 tests. Checksums are in `SHA256SUMS.txt`.
