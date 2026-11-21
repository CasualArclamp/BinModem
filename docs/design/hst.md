# HST (USRobotics High Speed Technology)

A fork-only addition. HST was never published; everything here comes from
reading the firmware of a Courier V.Everything (flash build 06/09/95). The
working notes with every address live outside this repository.

## What HST is, as the firmware does it

- Two channels on one two-wire line, at once. The **forward channel** carries
  2400 baud on 1800 Hz with V.32-family constellations: 4800 uncoded, 7200 to
  14 400 trellis coded exactly as V.32bis, 16 800 on a 256-point set. The
  **back channel** is differential 8-PSK, three bits a symbol, at 150 baud on
  375 Hz (450 bit/s) or 100 baud on 350 Hz (300 bit/s). The Courier's forward
  pulse is sharp (36 dB down 1400 Hz off the carrier), which leaves the back
  channel's band clear.
- Bits go least significant first. Each DSP word carries one symbol's bits to
  the 80188, which does HDLC framing in software: 0x7E flags, zero insertion,
  CRC-16/X.25, the same framing V.42 uses.
- Error control is MNP inside those frames, with USR additions: a link
  request parameter 0xC0 that agrees HST and the forward rates, optimized
  headers once connected, short data frames, and attention frames that hand
  the forward channel between the ends and change its rate on the fly.
- A call: the answerer sends 2100 Hz and then the forward channel; the caller
  hears 2100 Hz and answers on the back channel. The answering end sends fast
  first.

## What is built so far (milestone 1)

`datapump::hst`:

- `ForwardTx` / `ForwardRx`: the forward channel on the existing V.17 engine,
  which is V.32bis's modulation one way with a fixed training. A tenth-order
  560 Hz high-pass guard sits on the transmitter's output and in front of the
  receiver, to keep the two channels apart. Rates 7200 to 14 400.
- `BackTx` / `BackRx`: the back channel from the firmware's description:
  phase-step table, scrambler 1 + x^-14 + x^-17, both speeds. The pulse
  (root-raised-cosine, 35%) and the 60-symbol preamble are this
  implementation's own.

`crates/datapump/tests/hst_duplex.rs` runs both channels at once over a
simulated two-wire line with loss, hybrid echo and noise, at every rate and
both back-channel speeds, and carries HDLC frames both ways.

## Not yet

- Interoperating with a real Courier: the forward channel's start-up is
  V.17's 1.4 s train rather than HST's (180 + 72 + 200 or 3096 symbols), its
  pulse is V.17's rather than the Courier's, and the back channel's pulse and
  preamble are guesses. 4800 and 16 800 are missing.
- MNP error control with USR's additions, channel turnaround and fallback,
  the call-start handshake, and wiring into the modem and AT layers.
