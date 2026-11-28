# The window, the SIP caller and the dialler

[Back to the README](../../README.md)

## The window

The window is the faceplate of a modem. It shows the lamps, the level, the
rate of the connection, and the keys that dial and answer.

Below the faceplate are:

- the terminal, with a strip that shows how far the call has got and how long
  each step took
- a transcript of every frame that the two ends sent
- the scopes

The scopes have a card that names the signal that the receiver slices, and its
margin. The card then goes down the receive chain one stage at a time. Adjacent
to the card are the constellation, an oscilloscope, a waterfall and a spectrum.
These label their own peak, noise floor and occupied band.

A recording of a call between other parties replays through the same scopes,
as far as one tap on the line permits. See [review.md](review.md).

The panel also shows what the echo canceller removes, and where on the line it
found the reflection. On a two-wire pair the echo decides everything, and
without the panel you cannot see it. A constellation full of noise looks the
same if the noise is the line or the echo of this modem.

### The terminal: send menu and character format

Hold Ctrl and right-click the terminal to open the send menu. A right click
without Ctrl goes to the far end, as before. The menu has these items:

- **Break (250 ms)** and **Hard break (3.5 s)**. On a V.42 call, the modem
  sends a V.42 BRK message (8.13) with the length. On a call without error
  control, the modem holds the line at space for the length. On Telnet, both
  send `IAC BRK`. The transcript records how the modem sent the break.
- **Control characters**: NUL to US, and DEL, with their names.
- **Telnet commands** (Telnet only): IP, AO, AYT, EC, EL and NOP.

The box after the font slider sets the character format: 8N1, 7N1, 7E1, 7O1,
7M1 or 7S1. A 7-bit format removes the eighth bit from received characters.
It also puts the parity bit in the eighth bit of sent characters. The window
keeps the choice in `settings.txt` as `char_format`. 8E1 and 8O1 are not
available, because they need 11 bits for each character.

### Dark and light

The key at the right end of the tab strip sets the theme:

- ◑ follows the Windows theme. This is the default.
- 🌙 is always dark.
- ☀ is always light.

Click the key to go to the next theme. The window keeps the choice in
`settings.txt` as `theme system`, `theme dark` or `theme light`. In the light
theme, the faceplate, the readout, the scopes and the terminal stay dark.
DialSnoop is always dark.

## The SIP caller

BinModem now has a SIP caller. It is written as all other parts are: against
RFC 3261, 3550 and 4566, with no third-party stack. Thus you can place a call
without a virtual cable and a softphone in the path.

That middle part of the path did three things that are bad for a modem:

- It resampled 48 kHz to 8 and back.
- It ran an adaptive jitter buffer that inserts approximately 20 ms of audio
  every few seconds.
- It applied gain control.

The first damages the codewords of a V.90 server, which are the actual symbols
of the far end. The second was measured: it put a V.34 receiver out of lock
for 1.3 seconds.

Without the softphone the codewords arrive as they were sent. BinModem counts
the losses that stay and does not conceal them. Thus a failed call now tells
you if it failed in the modem or in the network.

## The dialler

BinModem has its own dialler: a keypad, a number box, a Call button, and a
form for the account. The form has the SIP server, the username, the password,
and UDP or TCP. Thus you can set up a trunk without an edit to a file.

Call types `ATD` at the terminal and does not go around the terminal. Thus
only one path places a call, and you can watch it.

T.38 is not there yet. BinModem declines a far end that asks for T.38. The
call stays on G.711, where the fax modulations already work.

The SIP caller was compiled and tested against a fake trunk. That trunk
challenges, refuses, retransmits and renegotiates. Since 2026-09-23 it also
works over a real trunk: it registers, places a call and carries the call in
mu-law. The modem works over that call. TCP was used only against the fake
trunk until now.

[Calling over SIP, with no softphone](../usage.md#calling-over-sip-with-no-softphone)
gives the procedure.
