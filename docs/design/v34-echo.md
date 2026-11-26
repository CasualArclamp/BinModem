# V.34 on one pair of wires: the echo canceller

The two directions of a V.34 call use the same band. At 3429 symbols/s the
two carriers are 1829 Hz and 1959 Hz, and each direction fills almost all of
the band between 200 Hz and 3700 Hz. A different carrier in each direction
does not separate them. On a line that brings back the signal of this end,
only an echo canceller can.

V.34 had no canceller. In simulation, with two modems on one line:

| Echo, against the far end | Before | Now |
|---|---|---|
| none | 33 600 bit/s | 33 600 bit/s |
| 30 dB down | 26 400 bit/s | 33 600 bit/s |
| 20 dB down | 14 400 bit/s | 33 600 bit/s |
| 10 dB down | `NO CARRIER` | 33 600 bit/s |
| the same level | `NO CARRIER` | 33 600 bit/s |

The last two rows are for an echo that arrives with the far end, as on one
shared cable. See [What does not work](#what-does-not-work).

## How it works

The canceller is in `crates/datapump/src/v34/startup.rs`. It is the
canceller of V.32 (`dsp::EchoCanceller`) with three additions.

- **When it learns.** Each end sends its TRN of phase 3 for 1 s, and the far
  end is silent then (V.34 11.3.1). The canceller learns only in that time.
- **Where the echo is.** The first run of taps covers 8 ms, for a hybrid. In
  the first half of the TRN, `dsp::EchoFinder` looks for a reflection that
  is farther away, and a second run of taps of 4 ms goes there. The search
  goes to the round trip of phase 2, or to 120 ms if that is longer.
- **The start of the TRN is not used.** The answer modem stops its J when it
  hears the S of the call modem. That silence arrives a round trip later,
  and the J can be in the first part of the TRN of the call modem.

### The fit

The least-mean-squares rule left the echo 33 dB down after 1 s. That is
because the signal does not fill its band equally, and the rule learns the
weak parts of the band slowly. 33 dB is not sufficient for 33 600 bit/s when
the echo is as loud as the far end.

`EchoCanceller::fit` solves the same equations directly from a recording of
the TRN. In simulation it leaves nothing of the echo (more than 100 dB). The
new taps are used only if they leave less of the recording than the old
taps.

### An echo that moves

On a sound card with two clocks, the echo arrives later and later. Taps that
do not move are of no use after some seconds. Taps that follow the echo read
between samples, and that has a cost: 35 dB to 40 dB, not 53 dB.

Thus two cancellers run. One keeps its taps where the fit put them. The
other is a copy that follows drift. Each 0.5 s, the modem compares what each
one leaves of the line. If the copy leaves less in two comparisons in
sequence, the copy becomes the canceller.

In simulation with an echo that moves at 20 parts per million, the call
stays connected, at 9600 bit/s and 24 000 bit/s.

### A line with no echo

A canceller that trained on a line with no echo learned noise. If it
removed less than 1 dB in its training, its taps go back to zero. That is
the rule of V.32.

## V.8: a CJ that did not arrive

CJ is three octets of zeros that the caller sends one time. With a loud
echo, the answer modem read the first two through its own JM and not the
third. It then sent JM until the call ended.

The answer modem now accepts two octets of zeros and then no octet for
67 ms as CJ. No menu has two octets of zeros together.

## What does not work

- **An echo that is loud and does not arrive with the far end.** In
  simulation, an echo at the level of the far end connects at 33 600 bit/s
  when its delay is 41 ms, 44 ms or 76 ms, with the far end 44 ms away. It
  does not connect at 19 ms, 31 ms, 38 ms, 46 ms or 56 ms. The canceller
  fits the echo fully in those calls. The faults are before and after it:
  the ranging of phase 2 gives an incorrect round trip on the first
  attempt, and the call modem does not find the S of the answer modem in
  phase 4.
- V.90 and V.92 have no canceller.
- A real telephone line with a hybrid was not tested.

## Tests

`crates/modem/tests/v34_two_wire.rs`: an echo as loud as the far end, a
faint echo, a hybrid, a line with no echo, and an echo that moves.
