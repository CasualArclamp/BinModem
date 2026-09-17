# BinModem

<img width="1314" height="892" alt="image" src="https://github.com/user-attachments/assets/f07ab50e-662e-4474-bae8-eda9861fb87a" />

A dial-up softmodem in Rust. Everything a modem does — the tones, the
handshakes, the error control, the compression — is code, and the line is a
sound card. No DSP chip, no driver blob, and none of a winmodem's dependence on
one vendor's Windows.

Written against the ITU-T Recommendations, with clause numbers cited in the
source for every normative constant, and against the RFCs for everything
carried over the top of them. 1529 tests.

## What works

It places and answers real calls. Full interactive sessions — public dial-up
gateways, reached over a VoIP trunk — have crossed it at V.22bis 2400 and at
V.32 4800, with V.42 error control and V.42bis compression, byte for byte
correct including the ANSI. Where a far end answers no XID at all and simply
announces compression in band, that is followed too, and 1957 octets of one
board's screen are kept as a test vector.

V.32bis runs to 14 400. One trellis code serves 7200, 9600, 12 000 and
14 400 — Figure 1/V.32bis draws it once, with four parallel lines each
labelled by the rates it exists for — and only the number of bits riding
through untouched changes, from one to four. The redundant bit buys nine
decibels at every one of them, which is why 14 400 fits in the channel 4800
does.

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
| **Commands** | V.250 AT: `+MS`, `+ES`, `+DS`, `+ER`, `+DR`, S-registers, `+++` |
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

- **V.32 at 9600** connects to a real modem, agrees on the trellis code and
  passes data, but not reliably. The receiver reaches 30 dB about half a
  second after connecting and then loses the carrier: the error while it goes
  is all across the radius and none along it, and the equaliser — frozen at
  the peak to check — leaves the radial part untouched. So it is the carrier
  loop, not the line and not the equaliser. Two calls now say the same thing.
  The rates above it are more crowded still and will want the same fix.
- **14 400 to a real modem, and it cannot be read.** Reached over the VoIP
  trunk once the two start-up faults below were fixed: both ends offered every
  rate, agreed on 14 400 and came up. The receiver locked for a second at a
  fifth of the distance between neighbouring points, then let go and settled at
  two fifths, which is where a symbol lands when the decisions are random. It
  stayed there for thirty-seven seconds. On one virtual cable, where it works,
  both ends carry V.42 and V.42bis at 14 400 with 0.76 of everything each modem
  says coming back a sixth of a second later, so what the trunk adds is the
  difference.
- **A rate that cannot be read is given up now.** 7 begins a retrain on
  "detection of unsatisfactory signal reception" and leaves the definition
  open. Ours was a distance, which cannot work: normalised the same way,
  neighbouring points are 1.41 apart at 4800 and 0.22 at 14 400, so one number
  was a quarter of the gap at one end of the range and one and a half gaps at
  the other — further than a symbol can land from the nearest point. The test
  was unreachable exactly where it was needed, which is why that call sat there
  for thirty-seven seconds. It is a fraction of the gap now, a quarter, which
  is what the old number was at the rate it was tuned on. The retrain that
  follows offers less than it did, since the rate exchange has no memory and
  would otherwise arrive back where it started; 5.4.1 and 5.4.2 both ask the
  rate signals to "take account of the likely receiver performance with the
  particular GSTN connection", and a rate this receiver has just spent a second
  failing to read is the strongest evidence about the connection there is.
  Tried on the line: 14 400 came up, was unreadable, and the modem retrained
  and agreed 12 000 with the far end — the first rate renegotiated with a real
  modem. 12 000 was unreadable too, so the step is chosen from the measurement
  now rather than taken one rate at a time; on a line with a second's delay a
  retrain costs fifteen seconds and the far end hung up during the third.
- **The rate signal gets misread, and it costs the rate.** One recorded call
  to a real V.32bis modem: the far end offered 4800 through 14 400 and sent
  that same sixteen bits 201 times; this end read one corrupted copy of it,
  answered with an E calling for 4800, and connected there. 5.3.1 asks for two
  consecutive identical sequences and two is what a systematic misread
  produces — counted at the locked phase, the true sequence ran 53 consecutive
  and every wrong one ran once, except the one that was acted on, which ran
  twice. Waiting for a third was tried and is worse: on a line returning the
  transmitter at unity a correct reading never happens three times running, so
  a modem that holds out never connects. The reading wants a better receiver
  under it, not a stricter test above it.
  [tools/read_rate_signals.py](tools/read_rate_signals.py) reads them off a
  recording.
- **The round trip is a second and a quarter, and V.32's start-up has no room
  for it.** MicroSIP to a real modem measures 2947 symbols there and back. Two
  faults that only show up at that length have been found and fixed. 5.4.1's NT
  and 5.4.2's MT are counter readings the two ends compare, and this end was
  handing the pre-roll a trimmed version of one: the far end heard the S, ceased
  transmitting as 5.4.2 tells it to, looked again MT later and found the S had
  ended 30 ms earlier. It waited five seconds for another and then started the
  call from the answer tone, twice. And 5.4.1's "transmission of R2 shall
  continue until an incoming rate signal R3 is detected" was read as written,
  so this end talked at a modem that had gone back to the beginning for
  twenty-three seconds and let it hang up. Both are guarded by tests now;
  neither has been tried on the line again yet.
- **Two of these on one virtual cable** fail in one direction only. The
  answering end reads a clean thirty-two point constellation and brings up
  V.42 and V.42bis; the originating end sees noise. Measured off a recording,
  the originating end's own signal comes back at 121 ms and is 0.57 correlated
  with everything it hears, which is the asymmetry: only the calling modem has
  to receive the far end's second training segment while transmitting.

## Next

V.34 on a real call. A live call has reached data mode with a real modem at
31 200 towards this end: its login banner came through without an error, and the
far end heard this end's 33 600 well enough to ask, six seconds in, for a rate
renegotiation down to 28 800. That renegotiation is now answered -- two of
these renegotiate from either end and carry V.42 on through it, and the capture
replays through the modem to the banner on the terminal and the far end's new
MP read. A second call lost the far end's E to a VoIP slip; that capture now
connects in replay, the receiver re-timing itself to the slip and finding the
frames again from the superframe's bit inversions, and doing the same for
another slip once connected. And a full retrain: a far end that gives up on a
renegotiation, or meets a line that changed too much for one, falls back to
sending its tone and starting phase 2 again, and two of these now go back
through it together and come up at whatever the line will carry, without
exchanging their capabilities a second time.

V.90 on a real call. The analogue half is written from the Recommendation and
checked against a recording of a real server: its CM and JM, INFO0d, INFO1d,
Ja, TRN1d, Jd and the DIL all read back as they should, and the DIL fits what
the analogue modem asked for. Against a simulated server over a simulated
G.711 network -- A-law and μ-law, a robbed bit, a pad, noise, a VoIP round
trip, a sound card 120 ppm out, and a jitter buffer slipping every few seconds
-- it connects at 52 000 to 56 000, renegotiates and retrains from either end,
and carries V.42 at `AT+MS=V90`. The first live call reached the DIL against a
real server and showed two things the simulation did not have: a jitter buffer
cutting ten milliseconds out of the DIL on every pass, and a gain control
holding loud codewords down. Both are now simulated, and followed.

Next, V.17 for fax at 14 400, and V.33 beside it -- the same trellis code as
V.32bis again, on a fax call and a leased line respectively -- and more than
one page to send from the fax window.

Alongside them: MNP as an alternative to LAPM, since it is what a modem without
V.42 will offer.

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
