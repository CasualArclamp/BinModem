# Architecture

## Goal

A standards-compliant voiceband modem, in Rust, that interoperates with real
modems up to ITU-T V.34 (33 600 bps), with V.42 error control, V.42bis
compression and a full V.250 AT command set, presented to Windows as a COM port.

## Decisions

| Area | Decision | Rationale |
|---|---|---|
| Language | Rust, `x86_64-pc-windows-msvc` | Sample-rate adaptive loops need compiled speed; MSVC is the default Windows target with the fewest crate surprises |
| Ceiling | V.34 / V.34+ (33.6k) | V.90/V.92 downstream is physically impossible between two analog endpoints, so it is out of scope as a *transmit* mode |
| Line side | Sound card into VB-Cable into a softphone | Local softswitch for development; an outbound SIP trunk to reach real answering modems later |
| DTE side | com0com virtual COM pair | Lets Windows Dial-Up Networking bind to us, which is the end goal. A TCP/stdio DTE comes first so the AT layer is testable without drivers |
| Error control | V.42 LAPM + V.42bis | Required for `CONNECT 33600/V42BIS` and for credible interop |
| Specs | ITU-T PDFs vendored in `docs/specs/` | Every magic constant must cite a clause number |
| Verification | Golden vectors from real captures + unit tests | See `tests/vectors/README.md` |

## The rule that shapes everything: streaming, not blocks

The previous attempt (`F:/dialupmodem`) defined its modem interface as
`modulate(bits) -> samples` and `demodulate(samples) -> bits`. That shape is
the reason it stalled below V.32.

A real modem is a set of continuous adaptive control loops:

- symbol timing recovery
- carrier phase and frequency tracking
- adaptive equaliser (fractionally spaced, decision directed)
- echo canceller (near and far end)
- Viterbi decoder with a survivor path spanning many symbols

None of these may be reset at a buffer boundary. A block API forces
re-acquisition on every call, which is why the old engine accumulated
`_demod_with_timing`, `_condition` and `_cancel_own_echo` workarounds around
its own interface.

**Therefore:** no public API in `dsp` or `datapump` accepts or returns a block
of samples. Everything is `feed(sample) -> Option<output>`. Blocks exist only at
the audio device boundary, where the OS imposes them.

## Crates

```
crates/
  dsp/        streaming primitives: biquads, Butterworth design, NCO, FSK detection
  datapump/   modulations: Bell 103 today; V.21/V.22/V.22bis/V.32bis/V.34 to come
  line/       the physical side: WAV today; WASAPI, resampling, drift tracking later
```

Planned additions, in dependency order: `negotiate` (V.8/V.8bis, V.25 answer
tone, call progress), `ec` (V.42 LAPM, V.42bis BTLZ), `at` (V.250 interpreter,
S-registers, result codes), `dte` (COM port and flow control), `telemetry`
(lock-free frame publishing), `modem` (top-level state machine), `gui`.

## Milestones

Ordered so there is a usable modem at every rung, rather than nothing until the
end.

1. **Skeleton** — AT interpreter, DTE binding, V.42/V.42bis, live audio, all
   working end to end at 300 bps. This makes it a *modem* early; every later
   rung is a datapump swap inside a proven frame.
2. **V.21 / V.22 / V.22bis** — frequency-division duplex, so still no echo
   canceller. Adds passband QAM, RRC shaping, timing recovery, adaptive
   equalisation.
3. **V.32 / V.32bis** — the first hard jump. Both directions share one band, so
   this requires a real echo canceller, plus 8-state Wei trellis coding and a
   Viterbi decoder, and full V.8 negotiation.
4. **V.34** — line probing, shell mapping, precoding, nonlinear encoding,
   multidimensional trellis, variable symbol rates and carriers.
5. **Real interop** — outbound SIP trunk to an answering modem.

## Known hazards

- **VoIP is hostile to V.34.** G.711 gives about 35 dB SNR, and 33 600 bps needs
  roughly that with no margin, so 28 800 is the realistic ceiling over VoIP.
  Jitter-buffer adaptation causes sample slips that V.34 timing cannot survive;
  network echo cancellers destroy same-band duplex; VAD and comfort noise blank
  the carrier. The path must be G.711 passthrough with EC, VAD and adaptive
  jitter all disabled.
- **Clock drift.** The softphone's 8 kHz clock and our sound clock are
  independent. An asynchronous resampler with drift estimation belongs in `line`
  from the start, not bolted on later.
- **The reference captures are 2-wire.** Both directions are summed on one tap.
  For Bell 103 and V.22bis the directions are in separate bands and separable;
  for V.32 and above they overlap, so those vectors exercise our handshake
  detection and transmitter, not a clean receiver input.
