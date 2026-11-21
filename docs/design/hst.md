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

## Milestone 1: the two channels

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

## Milestone 2: MNP and the handover

`ec::mnp` is MNP error control as HST carries it: link request, disconnect,
numbered data frames, windowed acknowledgement with go-back-N retransmission,
attention and its acknowledgement. Link setup uses standard headers (the
request is the Courier's template byte for byte, plus USR's parameter 0xC0);
once both ends agree parameter 8 the link moves to the optimized headers the
Courier's receive path reads. Sans I/O, like `ec::lapm`.

`datapump::hst::Session` is one end of a call: both channels, HDLC, MNP, and
the policy on top.

- The answerer holds the forward channel first; the caller is the MNP
  originator and sends its request on the back channel. Everything starts at
  7200 and 450; once 0xC0 is agreed the forward channel retrains at the
  fastest rate both allow, and the back channel drops to 300 if either end
  asked.
- Handover: the back-channel end with more than `turnaround_backlog` octets
  waiting sends attention 0x1R. The holder acknowledges, lets that go, and
  moves to the back channel; the requester drops its back channel, waits
  400 ms and trains the forward one. A minimum hold stops ping-pong.
- Online fallback and fall-forward: the receiving end watches the forward
  receiver's signal to noise and asks for a new rate with attention 0x3R; the
  holder retrains at it. The Courier changes bits per symbol without
  retraining; the V.17 engine cannot, so this retrains.
- The forward-holding end listens for both back-channel speeds at once, so a
  change of speed needs no coordination.
- A watchdog puts the roles back (answerer forward, caller back) if a handover
  is half lost and an end hears nothing for 5 s.

`crates/datapump/tests/hst_session.rs` calls end to end over the simulated
two-wire line: connection and rate agreement, a download with typing coming
back on the back channel, an upload that takes the forward channel and a
download that takes it back, the 300 bit/s back channel, a noisy line that
falls back and recovers its lost frames (and, with fallback off, nearly
stalls), and a line that clears and climbs back to 14 400.

## Milestone 3: the modem and AT layers

`AT+MS=HST` selects it, and `+MS`'s minimum and maximum rates limit which
forward rates the link request offers (`AT+MS=HST,0,0,9600`). `ATA` and `ATD`
then run `datapump::hst::call::Call`:

- The answerer is quiet for V.25's billing delay (2 s), sends 2100 Hz for
  3.3 s, waits 75 ms, and starts the session holding the forward channel.
- The caller waits for the tone to have been there 400 ms and then gone
  30 ms, and starts the session holding the back channel; as MNP originator
  it sends the link request.
- `CONNECT 14400/ARQ/HST` (or the agreed rate) goes to both terminals once MNP
  is up, after `+ER: ALT` if `+ER=1`. Data mode, `+++` and `ATO` are the
  modem's own; typed octets go into MNP up to a 4 KB backlog.
- `ATH` sends MNP's LD and holds the line a second for it, and the far end
  hangs up on hearing it. A far end that goes silent instead is given up after
  12 s without its carrier, past the 5 s watchdog that puts the roles back.
- Nothing up 60 s after going off hook: `NO ANSWER` if no tone was heard,
  `NO CARRIER` if one was.

The tone lengths are V.25's, not measured from a Courier.

## Not yet

- Interoperating with a real Courier: the forward channel's start-up is
  V.17's 1.4 s train rather than HST's (180 + 72 + 200 or 3096 symbols), its
  pulse is V.17's rather than the Courier's, and the back channel's pulse and
  preamble are guesses. 4800 and 16 800 are missing. The handover and
  fallback timings are this implementation's.
- USR's other frame types (piggybacked acknowledgement, short data frames)
  and MNP class 5 compression.
- Automode on a dual-standard answer: falling back to V.32bis or V.22bis
  behind the tone when the caller is not HST.
- USR's own S-registers and `&N` link-rate commands.
