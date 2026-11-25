# How to read a recording of a call between other parties

[Back to the README](../../README.md)

`dialsnoop.exe recording.wav` reads a WAV of a call in which it took no part.
The Review tab of the modem window (`binmodem --review recording.wav`) does
the same.

## The display

- A waterfall shows the call, with time from left to right.
- Each signal has a box, with a few letters adjacent to it.
- A panel shows all that was read from the box that you click. These are its
  fields, its octets, and the symbols that a receiver made of it.
- The recording plays from the point that you click.
- Each signal has a key that plays only that signal.

## What it reads

It uses the receivers of the modem itself, with the modem removed. Thus it
reads what this modem would read at that end of the call:

- **Set-up**: calling and answer tones, V.8 bis signals and messages, V.8's
  CI, CM, JM and CJ, and V.92's quick-connect sequences.
- **Phase 2**: the INFO sequences, and tones A and B and their reversals. The
  line probing, with a level and signal to noise for each of its tones. The
  round trip from where the recording was made.
- **V.34**: S, PP, TRN, J, MP and E from each end, data mode in each
  direction at its own rate, rate renegotiations and retrains.
- **V.90**: the analogue modem's Ja with the DIL it asks for, CPt and CP. The
  digital modem's Sd, TRN1d, Jd, the DIL itself, Ri, TRN2d, MP, Ed and B1d.
  Data mode in both directions.
- **What it carried**: V.42's XID, SABME, UA and I frames, and what is in
  the frames as PPP or as text.

## How far it gets

The result depends on the recording.

Some recordings have one end on each channel: a capture of the two RTP streams
of a call, or a recording made by this modem. Then each end is almost alone on
its channel. The reader can follow each end into data mode and through it.

Other recordings have both ends on one channel, as from a tap on the pair or a
microphone at the speaker. Then it reads as far as the two ends share a band,
which is the end of phase 2.

## Tests and limits

In two tests, these modems call each other, on V.34 and on V.90. Each end
stays on its own channel, and the reader gets only the recording. The reader
gives what each terminal typed.

On recordings of real calls from 2016 it reads every handshake through the DIL
and the rate that the two ends agreed. On one of them it reads the login
prompt of the ISP from a 48 000 bit/s downstream.

It does not yet read most real V.90 data:

- The channel of the analogue modem contains the echo of the server, and no
  part of BinModem cancels that echo.
- Compressed data shows as it is in the frames and is not expanded.
- V.92's PCM upstream and the modulations below V.34 are not followed after
  set-up.
