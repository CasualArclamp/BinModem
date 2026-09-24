One file. Download `binmodem.exe` and run it. There is nothing to install, and no Visual C++ redistributable is needed because the C runtime is linked in.

```
binmodem.exe                  a modem on a real line
binmodem.exe --devices        what audio this machine has
binmodem.exe --telnet         a board over a socket
binmodem.exe --capture        replay the golden capture
```

[docs/usage.md](https://github.com/CasualArclamp/BinModem/blob/main/docs/usage.md) has the setup.

## What's new in 1.4.1

### V.32bis: a start-up that gave up 283 milliseconds too early

- **A far end that trains for as long as the Recommendation allows is waited for now.** 5.4.1 has R2 sent "until an incoming rate signal R3 is detected" and puts no limit on the waiting, so this end had to — and the limit it set gave the far end one round trip plus 8464 symbols. But 8464 symbols is exactly what a conditioning signal costs when its training segment is at 5.2.3's maximum, so the budget left the far end no time at all to read R2 in, and one at the top of that range missed the deadline by however long reading it took.
- **The deadline runs from the far end's conditioning signal instead,** which is the one thing here that is heard rather than calculated: from the start of one, a rate signal is 8464 symbols away at the outside and nothing else can be behind it. A far end audibly still working through 5.4.2 is not a far end doing something else.
- **Found on a real call.** A V.32bis call over a trunk reached the rate exchange and abandoned it 283 ms before the far end's R3 arrived — an R3 offering 14 400, which was the rate the two ends would have agreed on. What followed was worse than the miss: back to repeating state A, opposite a far end in a rate signal that would never alternate again, holding the line for the whole minute of patience. That recording now connects at 14 400, with the receiver 0.04 of the way to the next point and 39 dB of signal to noise.

### The line row picks a kind of line

- **`sound card` or `SIP`,** where the box used to list the SIP accounts by name. It was answering two questions at once — what sort of line this is, and whose it is — and named a trunk where it should have named a mechanism. Which account is the dialler's business, and the row shows it beside the box and reads it from there, so the two cannot fall out of step.
- **`Open` on SIP says why it is disabled** when there are no accounts yet. It used to be enabled and quietly do nothing.

## Also in 1.4.0, which went out with the previous release's notes by mistake

### A SIP caller of our own, so a call needs no softphone

- **The softphone is out of the path.** Reaching the telephone network through a virtual cable and MicroSIP meant resampling 48 kHz to 8 and back, an adaptive jitter buffer inserting about 20 ms of audio every few seconds, and gain control. The first held a V.90 server's codewords to about 37 dB where a clean path gives 82; the second was measured putting a V.34 receiver out of lock for 1.3 seconds and making a 43 dB line read as 17.
- **`crates/sip` replaces it:** RFC 3261 for the signalling, 3581 for rport, 3550 for the packets, 4566 and 3264 for what the two ends agree to, 1321 for the digest a registrar asks for, and G.711 for the samples. No third-party stack and no async runtime — std, threads, and the same rules as everything else here.
- **`sip::Line` offers the same receive and transmit as `line::Duplex`,** so the loop driving the modem does not know which kind of line it is on; and since no packets arrive before the far end answers, the modem is never stepped into a handshake with a call that has not connected.
- **UDP or TCP, chosen per account.** Over TCP a message is framed by its Content-Length and the retransmissions are off. Only G.711 is ever offered: every other codec in a trunk's list is a speech coder, and a modem signal does not survive being fitted to a model of a voice tract.
- **The jitter buffer is fixed rather than adaptive.** It never stretches or repeats audio, conceals only with silence, and counts every octet of it — so a failed call now says whether it failed in the modem or in the network, which could not be asked before.
- **In the window:** a keypad dialler, a credentials form, and Connect. Call types `ATD` at the terminal rather than reaching past it, so there is one path that places a call and it can be watched.

## Status

- **V.32's round-trip measurement can still be halved by a lost packet.** On the call above, the concealment brought the far end's alternation back inverted — a slip of about one symbol, which is exactly what turns an alternation over — and the reversal that came out of it stopped the clock at 706 ms on a line whose real one is 1.22 s. Nothing available to the start-up separates that from the far end turning over: both invert the sidebands by 180°, and the tone is back at full strength before either is declared. The call connects regardless now; what the bad reading still costs is the echo canceller's tap placement.
- **One ISP's V.90 pool sometimes stops dead** just as its modem takes up our constellation, at the end of phase 4 or in a renegotiation. Every CP we sent was checked against V.90 Table 14 bit for bit, and they are correct. The same server has also reached data mode at 54 666 and 56 000.
- **Known:** after a V.90 call falls back to V.34, the V.34 retrain watch can still take the hang-up beep for tone B.
- **Compression between two BinModems in V.90** is not yet offered correctly.
- **V.92:** planned and in progress on a branch; not in this release.
- **Not yet tried against real far ends:** V.17, and the new V.29 and V.27 ter receivers, have been measured against our own modems and simulated lines only. The V.32bis receiver has now placed a real call at 14 400.
- **Unchanged:** the fax window sends one page per call.

2000 tests. Checksums are in `SHA256SUMS.txt`.
