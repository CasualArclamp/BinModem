# BinModem

<img width="1314" height="892" alt="image" src="https://github.com/user-attachments/assets/f07ab50e-662e-4474-bae8-eda9861fb87a" />

A dial-up softmodem in Rust. Everything a modem does — the tones, the
handshakes, the error control, the compression — is code, and the line is a
sound card. No DSP chip, no driver blob, and none of a winmodem's dependence on
one vendor's Windows.

Written against the ITU-T Recommendations, with clause numbers cited in the
source for every normative constant, and against the RFCs for everything
carried over the top of them. 1580 tests.

## What works

It places and answers real calls. Every modulation in the table below has
connected to real modems and providers' servers over a VoIP trunk, from Bell 103
to V.90, and full interactive sessions on public dial-up gateways have crossed
it with V.42 error control and V.42bis compression, byte for byte correct
including the ANSI. Where a far end answers no XID at all and simply
announces compression in band, that is followed too, and 1957 octets of one
board's screen are kept as a test vector. No far end ever has answered one, in
any of twelve recorded calls, and reading those calls back through the whole
modem said why: our XID broke four of the Recommendation's rules at once, and
the worst of them put a length in front of the V.44 offer where 12.2.1.3 says
there is none, so no far end could read the offer and no conformant answer to
it would have been read here either. All four are fixed.

V.32bis runs to 14 400. One trellis code serves 7200, 9600, 12 000 and
14 400 — Figure 1/V.32bis draws it once, with four parallel lines each
labelled by the rates it exists for — and only the number of bits riding
through untouched changes, from one to four. The redundant bit buys nine
decibels at every one of them, which is why 14 400 fits in the channel 4800
does.

V.34 works on real calls. Through a VoIP trunk to real modems it has come up
at 28 800 to 33 600 and carried login banners and PPP without an error. The
recordings of those calls replay through the modem to the same banners, and in
them it answers the far end's rate renegotiation and follows the jitter
buffer's slips, both in the start-up and once connected. Phase 2 reads the far end's probe for 300 ms rather than the
full 500 V.34 allows, because a real modem that needed the difference to hear
our reply stalled three times running without it.

V.90 works both ways round. The analogue modem, the half that dials a
provider, reads a real server's start-up the way the Recommendation writes
it -- CM and JM, INFO0d and INFO1d, Ja, TRN1d, Jd and the DIL -- and connects
in PCM to real providers' servers over the VoIP trunk. The
digital modem, the half a provider runs, answers too, so two of these connect
at full PCM rates over a virtual cable. Against a simulated G.711 network --
A-law and μ-law, a robbed bit, a pad, noise, a second's round trip, a sound
card 120 ppm out, a jitter buffer slipping every few seconds and a softphone's
gain control -- it connects at 52 000 to 56 000, renegotiates and retrains from
either end, notices a far end that has hung up, and falls back to V.34 on a
route that would carry less as PCM. Where the top of the band is cut, as a
codec or a VoIP path cuts it, the analogue modem asks for spectral shaping the
way a real one does, so the server sends next to nothing where the cut is.

Every constellation was read off the figures by position rather than out of
extracted text, which loses the sign of each coordinate and shuffles the axis
labels through the rows. [tools/read_constellation.py](tools/read_constellation.py)
takes the scale from the spacing of the ticks and the origin from the
constellation's own quarter-turn symmetry. Run on V.32bis Figure 2-3 it gives
back exactly the thirty-two points read off V.32's Figure 3 by hand, which had
been checked three other ways: two documents, two methods, one table.

| | |
|---|---|
| **Modulations** | Bell 103 (300), V.22 (1200), V.22bis (1200/2400), V.32 (4800/9600, both codings), V.32bis (7200/12000/14400), V.34 (4800 to 33600: shell mapping, 16/32/64-state 4D trellis codes, precoding, non-linear encoding), V.90 analogue and digital modems (28000 to 56000 PCM down, V.34 up: DIL analysis, modulus encoding, spectral shaping, rate renegotiation; the digital one answers through a softphone) |
| **Negotiation** | V.8 CM/JM/CI/CJ and ANSam; V.25 answer tone told apart from it |
| **Error control** | V.42 LAPM — detection, HDLC, XID, REJ/SREJ, mod-128 |
| **Compression** | V.44 (LZJH) and V.42bis (BTLZ), both offered in one XID and the far end picks; V.42bis also followed in band |
| **Commands** | V.250 AT: `+MS`, `+ES`, `+DS`, `+DS44`, `+ER`, `+DR`, S-registers, `+++` |
| **Terminal** | ANSI/CP437 with mouse reporting; telnet (RFC 854) to use it alone |
| **Settings** | remembered between runs, so the modem comes back where it was left |
| **Files** | ZMODEM send and receive |
| **Fax** | T.30 group 3, sending and receiving; V.29 (7200/9600) and V.27 ter (2400/4800); T.4 Modified Huffman and Modified READ, T.6 MMR; T.30 Annex A error correction mode; any number of pages in a call, each drawn as it arrives |
| **Network** | PPP (RFC 1661/1662) with LCP, PAP and CHAP, and IPCP, Van Jacobson header compression (RFC 1144), and a ping over it; a dial-in login prompt, and a login script for dialling out |
| **Internet** | our own TCP (RFC 9293) and an HTTP/HTTPS proxy (RFC 9112): pages straight to the internet through a provider, or through a far BinModem that has it |
| **Line** | full-duplex sound card, or a WAV to replay |

It is a fax machine as well. A picture loaded in the Fax window becomes a
group 3 page and goes out under T.30; a public fax service over the same VoIP
trunk accepts the training check and confirms the page, and a page it sends
back arrives and saves as a PNG. V.29 carries it at 9600 and 7200, falling back
to V.27 ter at 4800 and 2400, and every point and training sequence of both was
read off the figures -- the extracted text turns the square root of two into
"2". What finally let a real machine's page in was twenty milliseconds of
silence that its transmitter puts in front of every training sequence on
purpose, and that this end had been taking for the end of the burst.

Between two of these the page goes under T.30's error correction mode:
numbered frames, and a partial page request for any the far end could not read.
A tenth of a second of loud noise dropped onto a page costs a few frames sent
again, and the page arrives exactly as it left; without error correction the
same noise prints as streaks. That in turn lets the page go in MMR, T.6's
coding, which has no end-of-line codes to find its place again by and comes to
under half the size of the Modified Huffman every machine reads. A page arriving
is drawn a row at a time as it comes in, the way slow-scan television is.

It dials a real provider. The Network panel brings up PPP, logs in if the
far end asks, is given an address, and an ICMP echo crosses and comes back
with a round trip on it. Tick *carry web traffic*, set the browser's HTTP
proxy to `127.0.0.1:8080` for http and https, and pages come straight from the
internet: the request is read on this machine and the connection goes to the
web server's own address, with this program's own TCP, through the
provider's router. Between two of these, the machine that answered offers its
internet instead, and the proxy finds out which kind of far end it has by
itself.

The answering end can be a dial-in server, the way a provider's modem pool
was. A caller gets a banner, `login:` and `Password:`, and a prompt where `ppp`
starts PPP; the calling end's **Log in, then PPP** answers those prompts by
itself. A dialler that skips the text and starts PPP straight away is asked for
the same account over CHAP or PAP (RFC 1994, RFC 1334), so something other than
another BinModem can dial in.

TCP is ours, written against RFC 9293: the eleven states, retransmission with
RFC 6298's estimator, Nagle, delayed acknowledgements, zero-window probing,
out-of-order reassembly, and RFC 5681's fast retransmit. Almost nothing in
the path belongs to the operating system — the browser's socket to the proxy,
and the name lookups — and there is no driver, no adapter, no route and no
administrator.

The tallest test does the whole of it against a simulated line: V.8, V.22bis,
V.42, V.42bis, PPP, IPCP, TCP, the proxy, and a real web server on the
loopback.
Connected six seconds into the call, network phase at seven, a page back at
2400 bit/s — and it runs without a sound card.

The window around it is a scope: waterfall, spectrum, constellation, LED
faceplate, decoded transcript, and a log of every frame both ends sent. What
the echo canceller is taking out, and where on the line it found the
reflection, are on the panel too — on a two-wire pair that decides everything
and is otherwise invisible, since a constellation full of noise looks the same
whether the noise is the line or this modem listening to itself.

Portable in principle — `cpal` for the audio, `eframe` for the window, no
OS-specific code — but only built and run on Windows so far.

## Work in progress

- **V.32bis between two of these on one virtual cable** fails in the calling
  direction only: the calling modem hears its own signal back while it has to
  read the far end's second training segment.
- **V.90 between two of these through softphones** falls back to V.34. The
  server's codewords are resampled on their way into its softphone and
  requantised, which leaves noise at about 1.7% of every codeword; a real
  provider's server puts exact codewords on the network, which is why a real
  modem gets 56k on the same line.

## Next

V.17 for fax at 14 400, and V.33 beside it -- the same trellis code as V.32bis
again, on a fax call and a leased line respectively -- and more than one page
to send from the fax window.

V.92, on a branch. Eighteen agents read the Recommendation clause by clause off
the rendered pages, with V.8, V.8 bis and V.250's `+P` commands beside it, and
four more read what the V.90 code here already does; the notes they wrote and
the plan built from them are in [docs/design/v92](docs/design/v92) -- 62 work
packages in 18 waves, each with the files it may touch and the tests that will
prove it. The first milestone is a spike that has to show PCM upstream can
carry data at all before any of the state machines are written, because sending
codewords *up* the line is the one thing V.90 never does. Two waves are built
and merged so far: the shared numbers, the V.92 sequences, the upstream
encoder, precoder and transmitter, the quick-connect signals and the
impairments a softphone puts on the upstream. Every package is written by one
agent and then read by another against the pages again, which is how the
project's own capture turned out to be worth more than expected: it is a quick
connect, with no V.8 in it at all, and one of its frames decodes byte for byte
into what the reading of the Recommendation had predicted. That modem declined
PCM upstream, so no recording here contains the upstream signals, and the tests
that would want one say so rather than pretending.

## Running it

```bash
cargo run -p gui --release -- --live
```

Or `dist.bat`, which leaves a single `dist\binmodem.exe` holding the scope, the
terminal, an answering board and a capture to replay, with nothing to install.
[docs/usage.md](docs/usage.md) has the rest — the AT interface, the live-line
setup, ZMODEM and the telnet terminal.

## Licence

GPL-3.0-or-later. The ITU-T Recommendations themselves are not redistributed
here; `tools/fetch_specs.sh` downloads them.
