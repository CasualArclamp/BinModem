One file. Download `binmodem.exe` and run it — nothing to install, and no
Visual C++ redistributable, because the C runtime is linked in. The build is
checked for that before it is published.

```
binmodem.exe                  a modem on a real line
binmodem.exe --devices        what audio this machine has
binmodem.exe --telnet         a board over a socket
binmodem.exe --capture        replay the golden capture
```

The window opens on a real line, because that is what the program is for.
Reaching a line outside the machine wants two virtual audio cables, so the
output can go to a softphone's microphone while the input comes from its
speaker — [docs/usage.md](https://github.com/CasualArclamp/BinModem/blob/main/docs/usage.md)
has the setup. `--capture` replays a real Bell 103 call instead and needs no
audio at all, which is the quickest way to see whether it runs.

Bell 103, V.22, V.22bis, V.32, V.32bis and V.34; V.8 negotiation, V.42 error
control and V.42bis compression; group 3 fax over V.29 and V.27 ter, with error
correction mode and MMR; PPP with PAP and CHAP; a V.250 AT interface, an
ANSI/CP437 terminal and ZMODEM.

New since v0.6.0: **something other than another BinModem can dial in**, and a
fax to a real machine falls back when the line will not carry the fastest rate.

- **A login in front of PPP, both ways round.** A call this end answers can
  meet what a dial-up provider showed: a banner, `login:`, `Password:`, and a
  prompt where `ppp` starts PPP, `help` lists the commands and `logout` hangs
  up. A dialler that skips the text and sends frames from the first octet — as
  Windows' Dial-Up Networking does — is answered as PPP and asked for the same
  account another way. The **Network** window has the one account, and **Log
  in, then PPP** on the calling end answers the far end's prompts by itself.
- **PAP and CHAP** (RFC 1334, RFC 1994, over RFC 1321's MD5, checked against
  its own test suite), in either direction. An end given an account asks callers
  for CHAP first and PAP after, never drops the demand when a far end refuses
  it, and ends the link with the reason on both ends when a password is wrong.
  A far end that got LCP and then nothing, because nothing here could answer its
  question, now gets an answer.
- **A fax that cannot train at the top rate steps down.** A real wired fax
  machine over a VoIP trunk offered 9600, could not train there, and answered
  the training check by re-sending its DIS rather than the failure-to-train the
  book asks for. This end had read every DIS as "start over" and commanded 9600
  again, five times, until the far end gave up. A repeated DIS after the command
  has gone is now read as the failure to train it is: 9600, 7200, 4800, 2400,
  and then a polite goodbye.

The PPP automaton was found to have four faults on the way to authenticating,
each now fixed and tested: its configure, terminate and code-reject packets went
out under the options the link had agreed rather than the defaults RFC 1661
requires, so a far end back at its own defaults could not read a hang-up or a
fresh request; a far end's Terminate-Request left this end stuck; a hang-up took
thirty seconds rather than six; a far end wanting a method this end lacks was
answered for ever; and a protocol this end does not run got silence rather than
the Protocol-Reject that stops a peer asking for half a minute.

V.34 does not yet follow a full retrain, which a far end falls back to when a
renegotiation goes unanswered or the line changes too much for one; the call
ends there instead. 33 600 wants about 35 dB of signal to noise and a VoIP line
gives 33 or 34, so a far end asking for 31 200 or 28 800 is the line, not the
modem.

Fax is one page per call, and V.17 at 14 400 is not written yet. The modem
stays in fax class after a fax call; `AT+FCLASS=0` makes it a modem again.

V.32 at 9600 still connects to a real modem and loses the carrier about half a
second in. `AT+MS=V32B,1,4800,14400` is what has been used for the calls that
stayed up.

Checksums are in `SHA256SUMS.txt`.
