# dialupmodem2

A standards-compliant voiceband softmodem in Rust, targeting ITU-T V.34
(33 600 bps) with V.42 error control, V.42bis compression and a V.250 AT command
set, presented to Windows as a COM port.

See [docs/design/architecture.md](docs/design/architecture.md) for the design and
the milestone ladder.

## Status

The skeleton is complete: a call can be placed from `ATD`, negotiated, brought
up over a real sound card, and carried with error control, and it comes back
out of the far end's `take_dte`. What is missing is speed - V.32bis and V.34 -
and the COM port that would let Windows dial it.

| Layer | State |
|---|---|
| `dsp` - biquads, Butterworth, NCO, FSK, FFT, shaping, timing, equaliser | working |
| `dsp` - split echo canceller, reflection finder, arbitrary-ratio resampler | working |
| `datapump` - Bell 103 at 300, transmit and receive, with its call setup | working |
| `datapump` - V.22bis at 1200 and 2400, with its handshake | working |
| `datapump` - V.32 at 4800 and 9600, with echo cancellation and its start-up | working |
| `ec` - V.42 detection, HDLC, LAPM, V.42bis, XID negotiation | working |
| `at` - V.250 command parsing, S-parameters, result codes, escape sequence | working |
| `modem` - the whole of one: AT to line, V.250 states, V.42 over either pump | working |
| `line` - WAV reader and writer, full-duplex audio | working |
| `telemetry` - frame publishing, transcript log, raw line-data channel | working |
| `terminal` - ANSI/CP437 screen emulator for BBS use | working |
| `gui` - waterfall, spectrum, symbol scope, faceplate, audio monitor, console | working |
| `gui --live` - a modem on a real line, with the terminal wired to it | working |
| V.32 trellis coding, V.32bis, V.34 | not started |
| DTE binding: a COM port for Windows Dial-Up Networking | not started |

A real V.22bis call has been placed through a virtual audio cable end to end,
connecting in 5.5 s and carrying data over V.42. V.32 does the same in
simulation across every loopback delay from 4 to 125 ms, and 300 bit/s does it
in 1.7 s with no handshake to speak of; neither has yet been tried on hardware.

Reaching anything outside this machine needs a second virtual cable, so the
output can go to a softphone's microphone while the input comes from its
speaker. One cable loops back on itself, which makes a fine two-wire line and
is no use for a call.

## Running it

Double-click `run.bat`, or from a shell:

```powershell
.
un.ps1
```

It offers a menu of the captures, builds, and launches. `-Vector v34-33600`
picks one directly, `-List` shows what is available, `-Dev` builds the debug
profile. `run.sh` does the same from Git Bash or Linux. Failing that,
`cargo run -p gui --release -- tests/vectors/bell103-300.wav` works directly.

To put a modem of your own on a real line instead of watching a recording,
the easy way is `run.ps1 -Live` (or `run.bat -Live`): it builds, offers a menu
of the machine's audio devices with a virtual cable picked by default, and
offers to start a second modem on the same line so there is something to dial.
`-Carrier B103|V22B|V32` chooses the modulation for both ends.

By hand:

```powershell
modem-scope --devices
modem-scope --live --in "<input device>" --out "<output device>"
```

The terminal in the window is then the modem's DTE. Type `AT` and it answers
`OK`; `AT+MS=B103` or `V22B` or `V32` chooses the modulation; `ATD` dials and
`+++` escapes back to command state. The scopes show the call as it happens
rather than a recording of somebody else's.

To have something to dial, run `modem-answer` on the same cable. It puts a
second modem on the line, answers, and echoes what is typed like the simplest
possible board:

```powershell
modem-answer --in "<input device>" --out "<output device>" --carrier V22B
```

One cable is right for this. What comes back from it is what was written to
it, summed with whatever else is writing, which is a two-wire pair with two
modems across it. Reaching anything *outside* the machine is the part that
needs a second cable.

Only the Bell 103 capture decodes to text so far; the rest still show their
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
