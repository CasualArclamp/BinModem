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
same cable. It puts a second modem on the line, answers, and echoes what is
typed like the simplest possible board:

```powershell
binmodem --answer --in "<input device>" --out "<output device>" --carrier V22B
```

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
is a frame that fails its check. A file transfer wants the stream for the same
reason, so the two refuse to run together. **Put it down** gives the terminal
back, and hanging up takes the link with it.

### Web traffic over it

Tick **carry web traffic** once the link is up and the two ends become a
proxy. The end that answered the call has the internet and offers it; the end
that dialled listens on `127.0.0.1:1080` and the panel says so. Point a
browser's SOCKS 5 proxy setting there — in Firefox, Settings → Network
Settings → Manual, SOCKS Host `127.0.0.1` port `1080`, SOCKS v5, and tick
*Proxy DNS when using SOCKS v5* so names are resolved at the far end where
there is something to resolve them with.

The loopback rather than every interface, deliberately: a proxy listening on
the network is one anybody on the network can use to reach the far end of
somebody else's telephone call.

What crosses is our own TCP (RFC 9293) over our own IP over PPP over the
modem. The only part of the path belonging to the operating system is the
socket the answering end opens to the site. Expect a page in tens of seconds
at 2400 and rather better at 9600; a modern page with a hundred requests on it
will not be pleasant, and a page from 1996 will be exactly as it was.

There is no authentication yet. Two of these go straight from LCP to
addresses; a far end that insists on PAP or CHAP will agree to LCP and stop
there, which is at least a failure with a name on it.

The clause numbers in `crates/ppp` are RFC numbers: 1662 for the framing, 1661
for the negotiation, 1332 for the addresses, and 791, 792 and 1071 for the
datagram, the echo and the checksum over both. `crates/tcp` is RFC 9293, with
6298 for the retransmission timer and 5681 for what to do about a loss;
`crates/socks` is RFC 1928.

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
