# Using BinModem

The long form. [README.md](../README.md) has what BinModem is and what state
it is in; this is how to drive it. The design and the milestone ladder are in
[design/architecture.md](design/architecture.md).

## Running it

Double-click `run.bat`, or from a shell:

```powershell
.\run.ps1
```

It offers a menu of the captures, builds, and launches. `-Vector v34-33600`
picks one directly, `-List` shows what is available, `-Dev` builds the debug
profile. `run.sh` does the same from Git Bash or Linux. Failing that,
`cargo run -p gui --release -- tests/vectors/bell103-300.wav` works directly.

To put a modem of your own on a real line instead of watching a recording,
double-click **`run-live.bat`** (or `./run-live.sh`). It builds, offers a menu
of the machine's audio devices with a virtual cable picked out by default, and
offers to start a second modem on the same line so there is something to dial.
Pass a modulation to set both ends: `run-live.bat -Carrier V32`, or
`./run-live.sh V32`.

One cable is right for that. What comes back from a cable is what was written
to it, summed with whatever else is writing, which is a two-wire pair with two
modems across it. Reaching anything *outside* the machine is the part that
needs a second cable, so the output can go to a softphone's microphone while
the input comes from its speaker.

By hand:

```powershell
binmodem --devices
binmodem --live --in "<input device>" --out "<output device>"
```

`--live` on its own opens with no line: the two devices are chosen in the
window, and Open joins them into one two-wire line. Changing either box moves
the call onto the new device.

The terminal in the window is the modem's DTE. Type `AT` and it answers `OK`;
`AT+MS=B103` or `V22B` or `V32` chooses the modulation; `ATD` originates, `ATA`
answers and `+++` escapes back to command state. The buttons above do exactly
those and nothing else — the modem has one interface, and a button that reached
past it would be able to ask for things a terminal could not. The scopes show
the call as it happens rather than a recording of somebody else's.

An ordinary `ATD` now negotiates before it starts. V.250 6.4.1 names the
mechanism — `<automode>` "enables or disables automatic modulation negotiation
(e.g., Annex A/V.32 bis or ITU-T Rec. V.8)" — and it is on by default, so the
two ends exchange call menus over V.21 and both enter the same modulation
instead of each guessing. `AT+MS=V22B,0` turns it off and means what it says.

**Advanced** beside the modulation box opens the rest of `AT+MS` — V.250
6.4.1's other three subparameters — as toggles and boxes rather than something
to type. It is mode aware: the line-rate boxes offer only the rates the chosen
modulation actually has, since asking Bell 103 for 2400 is not a slow
connection but an error. The command being composed is shown on the face of the
window, and Send types it; nothing here reaches past the AT interface.

A rate ceiling is the setting worth knowing about. Sixteen points at 2400 need
about 20 dB of signal to noise and four at 1200 need about 13, so on a line
that cannot give the first, `AT+MS=V22B,1,1200,1200` is not the slower
connection — it is the one that works.

To have something to dial, run the same program again with `--answer` on the
same cable. It puts a second modem on the line and answers the way a dial-up
provider did: a banner, `login:` and `Password:`, and a prompt where `ppp`
starts PPP. Log in as `guest` with no password, or give it an account of its
own:

```powershell
binmodem --answer --in "<input device>" --out "<output device>" --carrier V22B --user rory --password <password>
```

Everything the caller is shown is printed there too, along with who logged in,
the addresses PPP handed out and every ping that arrived.

## Trying V.34

**33600** in the rate row, or `AT+MS=V34`, offers V.34 in V.8 alongside
V.32bis and V.22bis. A far end without V.34 picks one of those and the call
goes ahead as usual.

A far end with it goes through V.34's start-up and into data mode. In phase 2
the two modems swap capabilities, measure the round trip with reversals of
tones A and B, probe the line with L1 and L2 in each direction, and settle
symbol rates and carriers. In phase 3 each end sends S, PP and TRN for the
other to train its equaliser on, and J to say what constellation it wants
next; in phase 4 they train again and swap MP sequences, which carry the data
rates, trellis code, shaping and precoding each wants. Then each sends B1 and
its data, and `CONNECT` gives the rate arriving, which can differ from the one
leaving. The terminal logs each step as it goes (`V.34 INFO0`, `V.34 ranging`,
`V.34 phase 3: training`, `V.34 phase 4: MP`, `V.34 data`), the constellation
scope shows the far end's points once phase 3 has trained the receiver, and
the far-end panel keeps what was found: the far end's capabilities, the round
trip, the probed line, how well each phase trained, the far clock and the VoIP
slips followed, what each end's J asked for, both MP sequences, and the rates
each way.

Data mode's constellation is hundreds of points -- 832 at 31 200, 1664 at
33 600 -- and the scope in the panel is too small to tell them apart. Click it
for a window of its own, as large as you drag it: it keeps the last few seconds
of symbols, so each point shows as a spot, and noise shows as spots that have
grown into each other.

Either end can go back to MP from data mode without starting again. A far end
whose receiver is struggling asks for a slower rate with a rate renegotiation
-- S, S-bar, TRN and a new MP -- and this end answers it at once and carries on
at the rates the new MPs come to, with `V.34 rate renegotiation` in the log
while it happens and no `NO CARRIER` or second `CONNECT`. A far end hanging up
with a cleardown ends the call the same way. The panel counts the
renegotiations and shows the rates they came to.

A far end without V.42 often has something to say the moment it connects -- a
login banner, a prompt. That is recognised as text rather than read as an
answer pattern, and goes to the terminal.

A VoIP call's jitter buffer now and then plays twenty milliseconds of audio it
made up, or drops some, and the receiver notices the jump and finds the signal
again. In the start-up that costs nothing. In data mode it costs a third of a
second or so: the receiver finds where the frames are again from the data
itself, by the bit inversions every superframe carries, and V.42 sends again
whatever was lost. The same search rescues a call whose far end's E was lost to
a slip, which would otherwise wait for it until it gave up. The panel counts the
times the frames were found again. Press **Record** first on any V.34 call that
goes wrong: the capture is what gets it fixed.

A capture can be drawn afterwards, too. The data pump's capture test dumps
every equalised symbol of one direction, and `tools/plot_constellation.py`
draws any stretches of it as constellations side by side -- training, data
mode, a renegotiation -- with instructions at the top of the script.

## Trying V.90

**56000** in the rate row, or `AT+MS=V90`, dials as the analogue half of V.90:
the end that calls an internet provider's modem pool. V.8 offers PCM alongside
V.34 and V.32bis, so a far end that is not a V.90 server picks V.34 or V.32bis
and the call goes ahead as one of those.

A V.90 server answers with its own PCM offer, and the start-up runs V.34's
phase 2 with V.90's INFO sequences in it: the server says what codec law and
power it sends at, and this end asks for its upstream symbol rate. In phase 3
the server trains this end's receiver with TRN1d, lists the downstream rates
it has in Jd, and plays a DIL -- every one of the 128 codeword magnitudes, six
frames each -- so that this end can see exactly what each codeword arrives as.
From that it picks the constellations for phase 4 and for data mode, spaced
for the noise it measured at the power the server may send, and in phase 4 the
two ends swap CP and MP and go to data. `CONNECT` gives the downstream rate,
from 28 000 to 56 000 in steps of 1333; the upstream is V.34 and the panel shows
it. The log goes `V.90 phase 2`, `V.90 phase 3: training`, `V.90 phase 3: Jd`,
`V.90 phase 3: DIL`, `V.90 phase 4`, `V.90 data`.

There is no I and Q to plot for PCM, so the constellation scope plots each
sample the modem reads against the next one. A clean line draws a grid: one row
and one column for each level in use, the dense middle being the quiet levels
and the loud ones spreading out towards the edges. Noise shows as the spots
growing into each other, a line that has lost its place as a cloud.

Once connected, the call keeps itself up:

- **Rate renegotiation.** Either end can ask for new rates without starting
  again. This end answers the server's at once, and asks for a slower
  downstream itself when the levels in use sit closer than about seven times the
  error it is reading them with. `V.90 rate renegotiation` shows in the log
  while it happens, with no `NO CARRIER` and no second `CONNECT`.
- **Retrain.** A server that retrains is followed back through phase 2, and so
  is a line this end cannot read at all.
- **Cleardown.** A server that ends the call politely ends it here as well.

PCM needs the digital path to arrive untouched, which a VoIP call does if the
softphone uses G.711 (PCMU or PCMA) and nothing on the way processes the audio.
In the softphone, allow only PCMU and PCMA, and turn off echo cancellation,
noise suppression, automatic gain and voice activity detection or silence
suppression. Set both volumes to 100%, and turn off Windows' audio enhancements
on the VB-Cable devices. The jitter buffer's slips are followed: during the
start-up, R and the DIL show where the frames went; in data mode, the frames
themselves do. Press **Record** before dialling: a capture of a V.90 call that
went wrong is what gets it fixed.

## Moving a file

The **Files** button opens ZMODEM: a path to send, a directory to receive
into, a progress bar, the rate, and the two numbers that say what the line is
costing -- rewinds, because an error is recovered by sending the sender back
over ground it had already covered, and subpackets that failed their check.

It is not an ITU Recommendation and there are no clause numbers to cite. The
reference is Chuck Forsberg's *The ZMODEM Inter Application File Transfer
Protocol*, October 1988, and the clause numbers in `crates/transfer` are its
own. Two values in it are given only by reference to a C header -- the frame
type numbers and the subpacket terminators -- and where the code relies on one
it says what it was derived from.

Tell the board to send first. This end answers; it does not ask.

## Sending a fax

The **Fax** button opens T.30. **Browse** loads a picture and makes a page of
it: 1728 pels across, at 3.85 or 7.7 lines per millimetre, thresholded or
dithered. The panel says what that page comes to and how long it takes at each
rate before anything is dialled.

**Send fax** types `AT+FCLASS=1` and then `ATD` with the number in the box. On
a softphone, place the call there first and press it once the far end has
answered; the calling tone goes out, the far end says what it is, and the page
follows. **Wait for a fax** types `AT+FCLASS=1` and `ATA` instead: this end
answers with 2100 Hz, says what it can receive, and keeps whatever page
arrives.

A page arriving is drawn as it comes in, a row at a time from the top the way
slow-scan television is, under **page arriving**. The view follows the newest
line, with a green line marking it; scrolling up to look at the top stops it
following until it is scrolled back down. With error correction the page draws
a block's frames at a time, and waits at a damaged frame until it has been sent
again. **Save as PNG** writes out the page, or as much of it as has arrived,
with each scan line drawn tall enough to make the pels square, which they are
not. **page to send** and **page arriving** both fold away, for a window that
is taller than the screen.

The **offer** boxes are what this end will use. V.29 carries a page at 9600 and
7200; V.27 ter at 4800 and 2400, and every fax machine has it. A call starts at
the fastest rate both ends have and drops a rung each time the far end refuses
the training check. Untick V.29 to hold a call to V.27 ter on a bad line.

**ECM** is T.30's error correction mode. The page goes in numbered frames, and
any the far end cannot read are asked for and sent again rather than printed as
streaks. It is used only when the far end offers it too -- the panel on the far
end's DIS says whether it does -- and the progress line says *with error
correction* while a call is using it. Untick it to see what the same line does
to a page without it.

A page goes in the smallest coding both ends have, and the progress line names
it: MMR, T.6's coding, when there is error correction; Modified READ when the far
end reads it; and Modified Huffman, which every machine reads, when nothing else
is shared. The **coded** row under the page to send gives its size in all three,
and the times beside each rate run from MMR's to Modified Huffman's.

The far end's number and what its DIS says appear as soon as they arrive,
whether or not a page follows. One page to a call.

The modem stays in fax class after a call, as V.250 asks, so `AT+FCLASS=0`
makes it a modem again before dialling a board.

## Two of them on a network

The **Network** button brings up PPP over a call that is already connected, so
the two ends stop being terminals and start being machines with addresses.

Bring the call up as usual, on both machines, then press **Bring PPP up** at
each end. The end that answered the call hands out the addresses and keeps
10.0.0.1; the end that dialled asks with the zeroes RFC 1332 3.3 makes the
question rather than an address, and is told it is 10.0.0.2. Nothing is
configured -- watch the "this end" line change on the calling machine when the
answer arrives. Then **Ping**, or *one a second*, and the round trip appears
underneath.

What to expect: about 140 ms at 9600 over a virtual cable, and rather more over
a VoIP trunk, which adds most of a second each way before the modem has done
anything. The transcript carries the same thing in words, one line per echo,
alongside every other layer's.

While the link is up it owns the byte stream: nothing typed reaches the far
end and nothing from the far end reaches the screen, because a PPP frame is
not something anybody wants on a terminal and a keystroke in the middle of one
is a frame that fails its check. What is typed is dropped, with a line in the
transcript saying so; put the link down first to type again. A file transfer
wants the stream for the same reason, so the two refuse to run together. **Put it down** gives the terminal back, and hanging up
takes the link with it.

### Logging in

The Network window has one account, a name and a password. It is what this
end logs in with when it calls, and what a caller has to give when this end
answers.

Tick **answer calls with a login prompt** on the machine that will answer. A
caller then meets what a dial-up provider showed: a banner, `login:`,
`Password:`, and a prompt. `ppp` there starts PPP, `help` lists the commands
and `logout` hangs up. The answering screen shows the session as the caller
sees it, without the password. Three wrong passwords or a minute without a
login put the call down, and so does the end of the PPP link.

A dialler that skips the text and sends PPP frames from the start is answered
as PPP, which is what Windows' Dial-Up Networking does unless it is told to
show a terminal. That caller has not logged in, so PPP asks for the same
account: CHAP first (RFC 1994), and PAP (RFC 1334) when the account has no
password, since CHAP needs a secret at least one octet long.

On the calling machine, **Log in, then PPP** does the login. It answers the far
end's `login:` and `Password:` prompts with the account, types the **then
type** command at the prompt after them -- `ppp` unless changed -- and starts
PPP when the far end's first frame arrives. The far end's text is on the screen
while it happens. A far end that refuses the password or asks for the login
again stops it, and the terminal comes back with the reason in the transcript.
**Bring PPP up** skips the text, and still gives the account to a far end that
asks for it over PAP or CHAP.

The password is kept between runs in BinModem's settings file, in plain text,
so make one up for this rather than reusing one.

The transcript says who logged in and how, and a link that ends says why: the
password was wrong, the far end wanted MS-CHAP, which this does not do, or the
far end put the link down.

### Web traffic over it

Tick **carry web traffic** on the machine that dialled. It is an HTTP proxy for
a browser on that machine, for http and https alike. In Firefox, go to
Settings → Network Settings → Manual, fill in the **HTTP Proxy** box, and tick
*Also use this proxy for HTTPS*:

    HTTP Proxy  127.0.0.1    Port 8080

A browser still set up for SOCKS gets nowhere. The transcript says so --
*127.0.0.1:6817 is set up for SOCKS; set it to use an HTTP proxy* -- and the
fix is the box above.

Where a page goes from there depends on what answered the call, and the proxy
finds out for itself when the link comes up (**pages go: find out**):

- **A provider** -- a modem pool, whose far end is a router. The dialling
  machine reads the browser's request itself and opens the connection straight
  to the web server's own address, with its own TCP, through the provider's
  router. https is a CONNECT, and what goes through the tunnel is never read.
  *pages go straight to the internet*.
- **Another BinModem** with **carry web traffic** ticked too. That machine has
  the internet, and offers it on port 1080 over the link; the dialling machine
  passes each browser connection across untouched. *pages go through the far
  BinModem's proxy*.

Finding out is a single connection offered to the far end's port 1080. A
BinModem answers it; a provider's router refuses it, or says nothing for eight
seconds. Browser connections that arrive while it is finding out wait for the
answer. **pages go** can also be set to either way outright.

The one thing that does not cross the call is looking up names. There is no
DNS client here: nothing in this program is written from memory of a
protocol, and the RFCs for DNS are not among the ones it was written from. So
the machine's own resolver is asked, over whatever connection the machine
already has, and only the address it gives is used. The connection to that
address goes over the modem. A name served from many places may be pointed
near the machine's own network rather than near the provider, but it is still
an address on the internet and the router carries it there. IPv4 only, since
the stack under the call is.

An HTTP proxy costs no round trips before the request: RFC 9112 3.2.2 puts the
whole target in the request line, so the first thing the browser says is
already the request, and the connection behind it is opened while the rest is
still arriving. It also keeps each connection between requests, so a second
page from the same site opens no second connection. On a call to a provider
the round trip is over a second, so both matter.

Firefox opens up to six connections per proxy by default, and on a link this
slow that is six lots of setting up at once rather than six lots of progress.
Setting `network.http.max-persistent-connections-per-proxy` to `2` in
`about:config` is worth doing.

The proxy listens on the loopback rather than every interface, deliberately:
a proxy listening on the network is one anybody on the network can use to
reach the far end of somebody else's telephone call.

#### Settings and what to watch

Beside **pages go** are the **port** the browser is pointed at, and the
**MRU**: the largest frame the far end is asked to send (RFC 1661 6.1). TCP's
segment size follows it -- RFC 9293 3.7.1 has the MSS option be "the effective
MTU minus the fixed IP and TCP headers", and nothing larger than the far end's
MRU is ever built, whatever a web server says it can take. 1500 suits web
pages on a long round trip, because a server's slow start counts segments.
Smaller answers typing sooner (RFC 1144 5.2). All three are read when a link
starts.

**link and IP** folds open to show what LCP agreed each way (MRU, character
map, whether address, control and protocol are compressed), the header
compression, and what has crossed: frames in, out and broken, datagrams and
their octets, datagrams dropped (not for this address, unreadable, or a
compressed header nothing could rebuild), datagrams too large for the far end
and so never sent, and ICMP errors from routers. Below that are the TCP MSS the
proxy asks for and sends at, how many names were looked up and not found, and
the octets to and from the browser.

**connections** lists every TCP connection over the link: what it is for, the
address it goes to, its state, RFC 6298's smoothed round trip and timeout,
segments sent again, the congestion window and segment size, and the octets
each way. A connection whose resends climb while nothing comes back is the
line; one stuck in SYN-SENT is a server not answering; a router's *host
unreachable* is in the transcript and ends the attempt at once.

On the machine that answered, **carry web traffic** offers its internet to a
BinModem that calls, and says *offering the internet at 10.0.0.1:1080*. The
setting is kept for the next link, so a machine answering with a login prompt
can offer it before anyone has called.

The transcript says whether the headers are being compressed — *ppp: headers
compressed both ways, 16 slots*. That is RFC 1144, and it matters more here
than almost anywhere: every TCP segment carries forty octets of IP and TCP
header, and on this link an acknowledgement is nothing else and a keystroke is
forty-one octets of which one is the keystroke. What crosses instead is three
or four, because almost nothing in a header changes between one segment and
the next and what does usually changes by exactly the amount of data that went
past. Measured on the same conversation with it turned off, thirty-six of the
forty go. A far end that will not do it says so and the link carries on
without it, which the transcript also says.

What crosses is our own TCP (RFC 9293) over our own IP over PPP over the
modem. The only parts of the path belonging to the operating system are the
browser's socket to the proxy, the name lookups, and, between two BinModems,
the socket the answering end opens to the site. Expect a page in tens of seconds
at 2400 and rather better at 9600; a modern page with a hundred requests on it
will not be pleasant, and a page from 1996 will be exactly as it was.

The clause numbers in `crates/ppp` are RFC numbers: 1662 for the framing, 1661
for the negotiation, 1334 and 1994 for PAP and CHAP with 1321's MD5 under it,
1332 for the addresses and for negotiating header compression, 1144 for the
compression itself, and 791, 792 and 1071 for the datagram, the echo and
the checksum over both. `crates/login` has no RFC behind it, because the text
before PPP never had one. `crates/tcp` is RFC 9293, with
6298 for the retransmission timer and 5681 for what to do about a loss;
`crates/http` is RFC 9112 with RFC 9110 for what a proxy may do to a message
as it goes past.

## One file

`dist.bat` (or `./dist.ps1`) builds a release and leaves
`dist\binmodem.exe`, which is the whole program: a modem, the scope around
it, the telnet terminal, the answering board, and a capture to replay. Nothing
beside it, and nothing to install.

It opens on a real line and opens the line, on the two VB-Audio cables if the
machine has them -- A carrying what the softphone plays, B carrying what this
modem says. A machine without them gets the picker and a person to fill it in,
because falling back to whatever device sorts first would put the handshake
through the speakers. `--capture` replays the golden vector instead, a
path replays any recording, and `--telnet` opens the terminal onto a socket.

The C runtime is linked in rather than depended on. Without that the binary
imports `vcruntime140.dll`, which ships with the Visual C++ redistributable and
not with Windows, so a machine that has never had a developer tool on it
answers a double-click with a dialog naming a DLL. Everything else it uses is
Windows itself — `mmdevapi` for the audio, `user32` and `gdi32` for the window,
`opengl32` for the drawing.

The capture is carried inside the file too. It used to be opened from a path
built out of the directory the program was compiled in, which worked on exactly
one computer.

## The terminal on its own

Double-click **`run-telnet.bat`** (or `./run-telnet.sh`), or:

```powershell
binmodem --telnet
binmodem --telnet vert.synchro.net
```

No modem, no line, no audio: a socket to a bulletin board, feeding the same
terminal a call would. There is nothing for the scopes to show, so the terminal
gets the whole window.

It is there because *the board looked wrong* has two causes over a call, and
they want opposite fixes. An escape byte the line dropped turns the sequence
after it into text on the screen; an escape sequence the terminal does not
implement does much the same thing. Over a socket every byte arrives, so
anything still wrong is the terminal's — and anything that draws correctly
here and badly over a call is the line's.

`log bytes` puts everything the board sends into the transcript as well as on
the screen, which is the pair worth having side by side when something draws
wrongly: what arrived, and what it drew.

Mouse reporting works in both windows, because both send what the terminal
owes the far end by the same route. Say yes when a board asks whether your
terminal supports it: presses, releases, dragging, the wheel and the modifier
keys, in the original encoding or the extended one, whichever the board asks
for. A move is only reported when the pointer changes cell, which is what
keeps it usable on a line carrying 300 bits a second.

The window reports the two negotiated options that decide whether any of it
looks right. **7-bit!** means the board would not agree to eight-bit data and
the CP437 art will arrive with its top bits stripped; **local echo** means the
board is not echoing and this end is doing it instead.

The Bell 103 and V.22bis captures decode to text; the rest still show their
handshakes on the waterfall, which is worth watching in its own right - the
V.34 probing tones are clearly visible around six seconds in.

## Scope

The window carries a waterfall, spectrum, ARDOP-style symbol scope, LED
faceplate and a decoded transcript of both directions of the 2-wire tap.
"Listen" plays the line audio out of a chosen output device.

The lower panel carries a BBS terminal wired to the AT interpreter. Click it and
type: `AT` answers `OK`, `ATD` any number replays the capture and renders the
decoded session, `+++` escapes back to command state. ANSI colour, cursor
control and CP437 box drawing are all handled, so period art renders correctly.

## Build

Needs Rust (MSVC toolchain).

```bash
cargo test --workspace
```

The integration test decodes `tests/vectors/bell103-300.wav`, a real Bell 103
call, and asserts the known plaintext of the session.

## Repository layout

```
crates/          the modem itself
docs/specs/      ITU-T Recommendations, fetched by tools/fetch_specs.sh
docs/design/     design notes
tests/vectors/   golden signals cut from real captures
tools/           Python analysis: spectrograms, segmentation, reference decoders
WAV/             source captures
```

## Reference material

`tools/fetch_specs.sh` downloads the in-force edition of each ITU-T
Recommendation we implement against into `docs/specs/` (27 documents: the V.x
modulation series, V.8/V.8bis negotiation, V.42/V.42bis/V.44, V.24/V.250 and
the V.56bis test methods). The PDFs are gitignored; run the script to populate
them.

Implementation code should cite clause numbers for normative constants, e.g.
`// V.22bis 2.4.2: scrambler polynomial for the calling modem`.

## Analysis tools

```bash
python tools/analyze_capture.py                 # segment a capture, fingerprint tones
python tools/spectrogram.py out.png             # annotated spectrogram
python tools/decode_bell103.py [start] [end]    # reference Bell 103 decoder
python tools/extract_vectors.py                 # regenerate tests/vectors
```

The Python decoders are references for cross-checking the Rust implementation,
not part of the modem.
