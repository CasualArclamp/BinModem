# dialupmodem2

A standards-compliant voiceband softmodem in Rust, targeting ITU-T V.34
(33 600 bps) with V.42 error control, V.42bis compression and a V.250 AT command
set, presented to Windows as a COM port.

See [docs/design/architecture.md](docs/design/architecture.md) for the design and
the milestone ladder.

## Status

Milestone 1, the skeleton. A streaming DSP core, a Bell 103 receiver that
decodes a real captured call, a V.250 AT command layer, and a live scope.

| Layer | State |
|---|---|
| `dsp` - biquads, Butterworth design, NCO, FSK discriminator, FFT/spectrum | working |
| `datapump` - Bell 103 / V.21 receiver, async framing | working |
| `at` - V.250 command parsing, S-parameters, result codes, escape sequence | working |
| `telemetry` - frame publishing, transcript log, raw line-data channel | working |
| `terminal` - ANSI/CP437 screen emulator for BBS use | working |
| `gui` - waterfall, spectrum, symbol scope, faceplate, audio monitor, console | working |
| `line` - WAV reader, audio output | working |
| Live audio input, V.42/V.42bis, DTE binding, V.22bis and above | not started |

## Scope

```bash
cargo run -p gui
```

Replays a golden vector in real time through the receiver: waterfall, spectrum,
ARDOP-style symbol scope, LED faceplate and a decoded transcript of both
directions. "Listen" plays the line audio out of a chosen output device. Pass a
path to run a different capture.

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
