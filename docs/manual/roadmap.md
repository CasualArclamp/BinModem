# Work in progress and next steps

[Back to the README](../../README.md)

## Work in progress

### V.90 between two BinModems through softphones

V.90 between two BinModems through softphones falls back to V.34. The
codewords of the server are resampled on their way into its softphone and
requantised. This leaves noise at approximately 1.7% of every codeword. The
server of a real provider puts exact codewords on the network. That is why a
real modem gets 56k on the same line.

### V.ARC

V.ARC is our own wideband mode. It is made for a sound card, and it runs on
a SIP call in each coding also. Both ends must be BinModem.

- It has a multicarrier modulation that is loaded one carrier at a time.
- It has QAM and baseband levels that fill the band of the card (`AT+VARC`
  sets their baud, code and grid by hand).
- `AT+MS=VARC` or the mode box of the window selects it.
- `ATD` and `ATA` place and answer the call.
- EasySip is a SIP line with no account: type the address of the other end.

Each V.ARC call has the same start-up, on a sound card, on EasySip and on a
SIP account:

1. V.8. Each end says in the non-standard category of its menu that it has
   V.ARC.
2. The V.ARC handshake at 200 bit/s in the voice band. Each end says what
   its sound cards are and what its line is made of.
3. The mode.

- A far end that has no V.ARC gets an ordinary call, from V.92 down. V.8
  agrees it in the same exchange.
- With `AT+A8E=,,,1`, V.8 bis comes first and lists V.ARC also.
  `AT+VARCBIS` lists it when `+MS` names an ordinary mode.
- Each mode runs on each SIP coding. In G.711 (PCMU or PCMA) the
  modulations run at 8 kHz: approximately 26 kbit/s for the multicarrier
  mode, 23 kbit/s for QAM and 22 kbit/s for the levels.
- Where both ends have the same coding, `AT+MS=VARC` uses the numbers of
  the call as the data: 200 kbit/s on linear samples at 16 kHz, 61 kbit/s
  on A-law, 57 kbit/s on mu-law. If the path changes the numbers, the call
  continues as the multicarrier modulation.

Results and behaviour:

- Through a real lead between a Realtek line output and its line input at
  192 kHz, it carried 1.4 Mbit/s with no incorrect octet. A call is one
  signal, the same on both channels.
- In the V.ARC handshake each end says what its sound cards are. Thus ends
  that are different agree on what to run and do not fail.
- It starts on a line about which it knows nothing. The first signal needs any
  2.3 kHz of the band. It loads only where the line is.
- Each end tells the other end what it hears. A sender that is too loud
  decreases its own level.
- An end that cannot hear the other end says so. The far end then decreases
  the speed of the modulation by halves until that end can hear. Codes from
  0.93 down to 0.25 are for lines with little margin. One direction went out
  of a loudspeaker and into a microphone: 20 kbit/s.
- `AT+MS=VARC2W` is the same mode on one pair of wires, with an echo
  canceller. Each end first learns its echo while it has the line to itself.
  Two ends that share a virtual cable connect at 327 kbit/s and pass 75 dB.

V.ARC works between two machines. The two-wire mode has never met a real
two-wire line, and no mode has run through a telephone network. See
[docs/design/v-arc.md](../design/v-arc.md).

## Next

### V.33

V.33 is next. It uses the same trellis code as V.32bis again, on a leased
line.

### V.92

V.92 carries data between two BinModems. Set `AT+MS=V92` at both ends with
`BINMODEM_V92_PCM_UP=1`. PCM then goes in both directions, 48 000 up and up to
56 000 down, over a pair of virtual cables.

V.92 has not carried a call with a real V.92 modem yet. The side that calls
still needs a line with no echo and one clock.

#### Quick connect

A second V.92 call to the same number starts up the short way (9.2 and 9.4,
"quick connect"). It uses:

- QC1a and QCA1d in place of the menus of V.8
- a Phase 2 with no line probe
- no ODP/ADP

This removes approximately three seconds from the sixteen. `BINMODEM_V92_QC=0`
turns it off and `always` asks every server.

Sometimes the server of a quick connect sends no INFO0d or tone B within a
round trip and 2.5 s of the hand-over. Then BinModem forgets the memo and
sends CM for a full start-up. If that gets no JM, it hangs up.
`BINMODEM_V92_TONEQ_MS=<ms>` ends TONEq early as a test.

#### The plan

Eighteen agents read the Recommendation clause by clause from the rendered
pages, with V.8, V.8 bis and the `+P` commands of V.250. Four more agents read
what the V.90 code already does. Their notes and the plan made from them are
in [docs/design/v92](../design/v92).

The plan has 62 work packages in 18 waves. Each package lists the files that
it can touch and the tests that will prove it.

The first milestone is a spike. It must show that PCM upstream can carry data
before any state machine is written. The reason is that V.90 never sends
codewords *up* the line.

Two waves are built and merged at this time:

- the shared numbers
- the V.92 sequences
- the upstream encoder, precoder and transmitter
- the quick-connect signals
- the impairments that a softphone puts on the upstream

One agent writes each package. A different agent then reads it against the
pages again. That procedure showed that the capture of the project has more
value than expected. The capture is a quick connect, with no V.8 in it. One of
its frames decodes byte for byte into what the study of the Recommendation
predicted.

That modem declined PCM upstream, so no recording here contains the upstream
signals. The tests that would need such a recording say so and do not pretend.
