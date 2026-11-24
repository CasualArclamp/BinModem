# The slower modes, before anything is changed

Produced by `crates/datapump/tests/lock_sweep.rs`, which is `#[ignore]`d:

```text
cargo test -p datapump --release --test lock_sweep -- --ignored --nocapture
```

Nothing outside `crates/datapump/tests/lock_sweep.rs` was changed to obtain any
number here. Read that file's module comment for the whole of the method; what
follows is enough to read the tables.

## The columns

| column | what it is |
|---|---|
| lock | from the far end's carrier first appearing on the line to the first correct bit of a run of 200 with nothing wrong in it |
| slicer SNR dB | mean \|decision\|² over mean \|received − decision\|², over the steady state, against the alphabet that was actually transmitted rather than the one the receiver believes in. Comparable across rates, which a residual error is not |
| margin | mean distance from a decision, over half the distance between the two closest points. One means the average symbol is sitting on the decision boundary |
| SER | fraction of consecutive groups of one symbol's worth of aligned bits carrying at least one wrong bit. For Bell 103, whose receiver hands out whole characters and no bit stream, a character error rate |
| BER | bit error rate from lock onwards. The acquisition transient is not counted in it: that is what the lock column is for |
| 1st err | from lock to the first wrong bit |
| carrier lost | whether the receiver ever declared the carrier gone after finding it, and for how long |
| slips | how many times the recovered stream jumped relative to what was sent. One is a receiver that lost symbols and carried on; one on an otherwise clean line is a timing loop slipping |

A mode **held** a case when it locked and its bit error rate from lock was under
one in a thousand. Every cell is the middle of three seeds by bit error rate, so
no single draw of one noise sequence decides a threshold.

## The line

In the order `evidence.md` §4.2 fixes: the far end's sampling clock (by
resampling, so this is a real clock offset and not the trick of lying to the
receiver about `fs`), then a carrier offset applied as a true single-sideband
shift through a 255-tap Hilbert transformer so the symbol rate is untouched,
then an echo, then a level step and a dropout, then seeded white Gaussian noise
scaled so its power in a 3.1 kHz band is the stated number of decibels below the
mean power of the arriving signal.

Three things to keep in mind when reading the echo, clock and round-trip rows.

- **No echo canceller is used anywhere here.** That matters for V.32 alone: both
  directions share the band and a real V.32 modem cancels its own echo before the
  receiver sees anything (`crates/datapump/src/v32/startup.rs`, exercised by
  `v32_loopback.rs:157`). The V.32 echo rows measure the bare receiver and
  understate what a whole V.32 modem does. Every other mode's echo is either out
  of band or a listener echo, and its rows mean what they say.
- **The echo is a talker echo for the duplex modes and a listener echo for the
  rest.** Bell 103, V.22, V.22 bis and V.32 transmit while they listen, so the
  echo is of this end's own transmitter. V.21 as T.30 uses it, V.29 and V.27 ter
  are half duplex and silent while receiving, so the only echo there can be is
  the far end's own signal off the far hybrid. Note that 0.57 at 120 ms is the
  project's measurement of its *virtual cable's* talker echo; as a listener echo
  it is a reflection at 4.9 dB down and 144 to 288 symbol periods out, which no
  equaliser in this project spans — they reach ±4 to ±17 ms.
- **A clock offset carries a proportional carrier offset with it**, because the
  far end's carrier comes off the same oscillator. At ±200 ppm that is ±0.48 Hz
  on a 2400 Hz carrier and ±0.36 Hz on an 1800 Hz one. That is what ±0.01% means
  and it is not contamination.

## What the Recommendations ask for

Each read from the rendered page, never from `docs/specs/text`, which loses
signs and columns.

| mode | carrier offset the receiver must take | clock |
|---|---|---|
| V.21 §3 (Fascicle VIII.1, p. 2) | "the demodulation equipment must tolerate drifts of ± 12 Hz between the frequencies received and their nominal values" | — |
| Bell 103 | not an ITU Recommendation. Its shift is ±100 Hz, which makes a few hertz of offset negligible by construction | — |
| V.22 2.6 (Fascicle VIII.1, p. 3) | "the receiver shall be able to accept errors of at least ± 7 Hz in the received frequencies" | 2.5.1: 1200 bit/s ± 0.01%, 600 baud ± 0.01% |
| V.22 bis 2.6 (Fascicle VIII.1, p. 4) | "The receiver shall be able to operate with received frequency offsets of up to ± 7 Hz." | 2.5.1: 1200/2400 bit/s ± 0.01%, 600 baud ± 0.01% |
| V.32 2.1 (Rec. V.32 (03/93), p. 1) | "The carrier frequency is to be 1800 ± 1 Hz … The receiver must be able to operate with received frequency offsets of up to ± 7 Hz." | 2.3: 2400 bauds ± 0.01% |
| V.32 bis 2.1 (Rec. V.32 bis, p. 1) | "The receiver must be able to operate with a maximum received frequency offset of up to ± 7 Hz." | 2.1: 2400 symbols/s ± 0.01% |
| V.29 §4 (Fascicle VIII.1, p. 4) | "the receiver must be able to accept errors of at least ± 7 Hz in the received signal frequency" | §3: 2400 bauds ± 0.01% |
| V.27 ter §3 (Fascicle VIII.1, p. 5) | "the receiver must be able to accept errors of at least ± 7 Hz in the received frequencies" | 2.3.2: 1600/1200 bauds ± 0.01% |

Two conforming ends may each be 0.01% out, so ±200 ppm is the worst two legal
modems can be apart. That is why the clock sweep stops there.

V.22 and V.22 bis at 1200 bit/s put the same waveform on the line — V.22 bis
2.5.2.2 nominates the one point V.22 uses "irrespective of the quadrant
concerned … This ensure compatibility with Recommendation V.22" — and this
project has one receiver for both. The only thing separating the two rows below
is whether the receiver was told the rate or left to work it out.

## Bell 103 300

300 baud, 1 bit to the symbol.

Arrival phase, clean line: **8/8** of the sample phases carried the payload ([0, 1, 2, 3, 4, 5, 6, 7] worked). Everything below was run at phase 0, the best of the eight.

| case | lock | slicer SNR dB | margin | SER | BER | 1st err | carrier lost | slips |
|---|---|---|---|---|---|---|---|---|
| clean | 385 ms | 25.9 | 0.05 | 0 | 0 | never | no | 0 |
| carrier -7 Hz | 393 ms | 22.8 | 0.07 | 0 | 0 | never | no | 0 |
| carrier -3 Hz | 393 ms | 25.5 | 0.05 | 0 | 0 | never | no | 0 |
| carrier -1 Hz | 393 ms | 26.0 | 0.05 | 0 | 0 | never | no | 0 |
| carrier +1 Hz | 393 ms | 25.3 | 0.05 | 0 | 0 | never | no | 0 |
| carrier +3 Hz | 393 ms | 23.9 | 0.06 | 0 | 0 | never | no | 0 |
| carrier +7 Hz | 393 ms | 20.7 | 0.09 | 0 | 0 | never | no | 0 |
| clock -200 ppm | 385 ms | 25.9 | 0.05 | 0 | 0 | never | no | 0 |
| clock -120 ppm | 385 ms | 25.9 | 0.05 | 0 | 0 | never | no | 0 |
| clock -50 ppm | 385 ms | 25.8 | 0.05 | 0 | 0 | never | no | 0 |
| clock +50 ppm | 385 ms | 25.8 | 0.05 | 0 | 0 | never | no | 0 |
| clock +120 ppm | 385 ms | 25.7 | 0.05 | 0 | 0 | never | no | 0 |
| clock +200 ppm | 385 ms | 25.6 | 0.05 | 0 | 0 | never | no | 0 |
| SNR 36 dB | 385 ms | 25.7 | 0.05 | 0 | 0 | never | no | 0 |
| SNR 33 dB | 385 ms | 25.7 | 0.05 | 0 | 0 | never | no | 0 |
| SNR 30 dB | 385 ms | 25.5 | 0.05 | 0 | 0 | never | no | 0 |
| SNR 27 dB | 385 ms | 25.3 | 0.05 | 0 | 0 | never | no | 0 |
| SNR 24 dB | 385 ms | 24.8 | 0.06 | 0 | 0 | never | no | 0 |
| SNR 21 dB | 385 ms | 24.1 | 0.06 | 0 | 0 | never | no | 0 |
| SNR 18 dB | 385 ms | 22.9 | 0.07 | 0 | 0 | never | no | 0 |
| SNR 15 dB | 385 ms | 21.2 | 0.09 | 0 | 0 | never | no | 0 |
| SNR 12 dB | 385 ms | 19.0 | 0.11 | 0 | 0 | never | no | 0 |
| SNR 9 dB | 385 ms | 16.5 | 0.15 | 0 | 0 | never | no | 0 |
| SNR 6 dB | 385 ms | 13.7 | 0.21 | 0 | 0 | never | no | 0 |
| echo 0.57 @ 120 ms | 385 ms | 25.8 | 0.05 | 0 | 0 | never | no | 0 |
| echo -25 dB @ 1.1 s | 385 ms | 25.9 | 0.05 | 0 | 0 | never | no | 0 |
| level +6 dB | 385 ms | 25.9 | 0.05 | 0 | 0 | never | no | 0 |
| level -6 dB | 385 ms | 25.9 | 0.05 | 0 | 0 | never | no | 0 |
| dropout 20 ms | 385 ms | 25.9 | 0.05 | 0.0327 | 0.0155 | 2533 ms | no | 2 |
| round trip 1.1 s | 385 ms | 25.9 | 0.05 | 0 | 0 | never | no | 0 |

Lowest signal-to-noise ratio that still carried the payload: **6 dB**.

## V.21 300

300 baud, 1 bit to the symbol.

Arrival phase, clean line: **8/8** of the sample phases carried the payload ([0, 1, 2, 3, 4, 5, 6, 7] worked). Everything below was run at phase 0, the best of the eight.

| case | lock | slicer SNR dB | margin | SER | BER | 1st err | carrier lost | slips |
|---|---|---|---|---|---|---|---|---|
| clean | 10 ms | 25.5 | 0.05 | 0 | 0 | never | no | 0 |
| carrier -7 Hz | 16 ms | 21.6 | 0.08 | 0 | 0 | never | no | 0 |
| carrier -3 Hz | 16 ms | 24.5 | 0.06 | 0 | 0 | never | no | 0 |
| carrier -1 Hz | 16 ms | 25.4 | 0.05 | 0 | 0 | never | no | 0 |
| carrier +1 Hz | 16 ms | 25.3 | 0.05 | 0 | 0 | never | no | 0 |
| carrier +3 Hz | 16 ms | 24.4 | 0.06 | 0 | 0 | never | no | 0 |
| carrier +7 Hz | 16 ms | 21.5 | 0.08 | 0 | 0 | never | no | 0 |
| clock -200 ppm | 10 ms | 25.6 | 0.05 | 0 | 0 | never | no | 0 |
| clock -120 ppm | 10 ms | 25.5 | 0.05 | 0 | 0 | never | no | 0 |
| clock -50 ppm | 10 ms | 25.5 | 0.05 | 0 | 0 | never | no | 0 |
| clock +50 ppm | 10 ms | 25.5 | 0.05 | 0 | 0 | never | no | 0 |
| clock +120 ppm | 10 ms | 25.5 | 0.05 | 0 | 0 | never | no | 0 |
| clock +200 ppm | 10 ms | 25.4 | 0.05 | 0 | 0 | never | no | 0 |
| SNR 36 dB | 10 ms | 25.5 | 0.05 | 0 | 0 | never | no | 0 |
| SNR 33 dB | 10 ms | 25.4 | 0.05 | 0 | 0 | never | no | 0 |
| SNR 30 dB | 10 ms | 25.3 | 0.05 | 0 | 0 | never | no | 0 |
| SNR 27 dB | 10 ms | 25.1 | 0.06 | 0 | 0 | never | no | 0 |
| SNR 24 dB | 10 ms | 24.8 | 0.06 | 0 | 0 | never | no | 0 |
| SNR 21 dB | 10 ms | 24.0 | 0.06 | 0 | 0 | never | no | 0 |
| SNR 18 dB | 10 ms | 22.9 | 0.07 | 0 | 0 | never | no | 0 |
| SNR 15 dB | 10 ms | 21.2 | 0.09 | 0 | 0 | never | no | 0 |
| SNR 12 dB | 9 ms | 19.0 | 0.11 | 0 | 0 | never | no | 0 |
| SNR 9 dB | 8 ms | 16.5 | 0.15 | 0 | 0 | never | no | 0 |
| SNR 6 dB | 8 ms | 13.6 | 0.21 | 0 | 0 | never | no | 0 |
| echo 0.57 @ 120 ms | 10 ms | 8.9 | 0.36 | 0 | 0 | never | no | 0 |
| echo -25 dB @ 1.1 s | 10 ms | 23.2 | 0.07 | 0 | 0 | never | no | 0 |
| level +6 dB | 10 ms | 25.5 | 0.05 | 0 | 0 | never | no | 0 |
| level -6 dB | 10 ms | 25.5 | 0.05 | 0 | 0 | never | no | 0 |
| dropout 20 ms | 10 ms | 14.5 | 0.19 | 0.0012 | 0.0012 | 2845 ms | no | 0 |
| round trip 1.1 s | 9 ms | 25.5 | 0.05 | 0 | 0 | never | no | 0 |

Lowest signal-to-noise ratio that still carried the payload: **6 dB**.

## V.22 1200

600 baud, 2 bits to the symbol.

Arrival phase, clean line: **7/8** of the sample phases carried the payload ([0, 2, 3, 4, 5, 6, 7] worked). Everything below was run at phase 0, the best of the eight.

| case | lock | slicer SNR dB | margin | SER | BER | 1st err | carrier lost | slips |
|---|---|---|---|---|---|---|---|---|
| clean | 466 ms | 40.8 | 0.01 | 0 | 0 | never | no | 0 |
| carrier -7 Hz | never | 5.6 | 0.68 | 0.1916 | 0.1093 | never | no | 0 |
| carrier -3 Hz | 255 ms | 13.1 | 0.30 | 0 | 0 | never | no | 0 |
| carrier -1 Hz | 274 ms | 32.6 | 0.03 | 0 | 0 | never | no | 0 |
| carrier +1 Hz | 259 ms | 34.0 | 0.03 | 0 | 0 | never | no | 0 |
| carrier +3 Hz | 259 ms | 13.6 | 0.28 | 0 | 0 | never | no | 0 |
| carrier +7 Hz | never | 5.5 | 0.68 | 0.1842 | 0.1084 | never | no | 0 |
| clock -200 ppm | 464 ms | 38.4 | 0.02 | 0 | 0 | never | no | 0 |
| clock -120 ppm | 464 ms | 40.2 | 0.01 | 0 | 0 | never | no | 0 |
| clock -50 ppm | 464 ms | 41.5 | 0.01 | 0 | 0 | never | no | 0 |
| clock +50 ppm | 466 ms | 36.5 | 0.02 | 0 | 0 | never | no | 0 |
| clock +120 ppm | never | 31.1 | 0.03 | 0.7188 | 0.4688 | never | no | 0 |
| clock +200 ppm | 467 ms | 37.2 | 0.02 | 0 | 0 | never | no | 0 |
| SNR 36 dB | 454 ms | 39.1 | 0.01 | 0 | 0 | never | no | 0 |
| SNR 33 dB | 454 ms | 37.7 | 0.02 | 0 | 0 | never | no | 0 |
| SNR 30 dB | 454 ms | 35.9 | 0.02 | 0 | 0 | never | no | 0 |
| SNR 27 dB | 458 ms | 31.9 | 0.03 | 0 | 0 | never | no | 0 |
| SNR 24 dB | 458 ms | 30.0 | 0.04 | 0 | 0 | never | no | 0 |
| SNR 21 dB | 458 ms | 27.6 | 0.05 | 0 | 0 | never | no | 0 |
| SNR 18 dB | 456 ms | 25.0 | 0.07 | 0 | 0 | never | no | 0 |
| SNR 15 dB | 453 ms | 22.4 | 0.10 | 0 | 0 | never | no | 0 |
| SNR 12 dB | 453 ms | 19.5 | 0.13 | 0 | 0 | never | no | 0 |
| SNR 9 dB | 453 ms | 16.5 | 0.19 | 0 | 0 | never | no | 0 |
| SNR 6 dB | 451 ms | 13.6 | 0.26 | 0 | 0 | never | no | 0 |
| echo 0.57 @ 120 ms | 466 ms | 40.8 | 0.01 | 0 | 0 | never | no | 0 |
| echo -25 dB @ 1.1 s | 466 ms | 40.8 | 0.01 | 0 | 0 | never | no | 0 |
| level +6 dB | 466 ms | 18.6 | 0.08 | 0 | 0 | never | no | 0 |
| level -6 dB | 466 ms | 18.6 | 0.09 | 0 | 0 | never | no | 0 |
| dropout 20 ms | 464 ms | 15.7 | 0.08 | 0.0193 | 0.0120 | 773 ms | no | 0 |
| round trip 1.1 s | 467 ms | 39.2 | 0.01 | 0 | 0 | never | no | 0 |

Lowest signal-to-noise ratio that still carried the payload: **6 dB**.

## V.22bis 1200

600 baud, 2 bits to the symbol.

Arrival phase, clean line: **8/8** of the sample phases carried the payload ([0, 1, 2, 3, 4, 5, 6, 7] worked). Everything below was run at phase 0, the best of the eight.

| case | lock | slicer SNR dB | margin | SER | BER | 1st err | carrier lost | slips |
|---|---|---|---|---|---|---|---|---|
| clean | 466 ms | 41.2 | 0.01 | 0 | 0 | never | no | 0 |
| carrier -7 Hz | never | 5.5 | 0.68 | 0.1200 | 0.0609 | never | no | 0 |
| carrier -3 Hz | 119 ms | 15.0 | 0.23 | 0 | 0 | never | no | 0 |
| carrier -1 Hz | 73 ms | 34.5 | 0.03 | 0 | 0 | never | no | 0 |
| carrier +1 Hz | 73 ms | 34.0 | 0.03 | 0 | 0 | never | no | 0 |
| carrier +3 Hz | 73 ms | 15.8 | 0.20 | 0 | 0 | never | no | 0 |
| carrier +7 Hz | never | 5.5 | 0.69 | 0.1256 | 0.0651 | never | no | 0 |
| clock -200 ppm | 464 ms | 38.3 | 0.02 | 0 | 0 | never | no | 0 |
| clock -120 ppm | 464 ms | 40.0 | 0.01 | 0 | 0 | never | no | 0 |
| clock -50 ppm | 464 ms | 41.3 | 0.01 | 0 | 0 | never | no | 0 |
| clock +50 ppm | 464 ms | 42.0 | 0.01 | 0 | 0 | never | no | 0 |
| clock +120 ppm | 464 ms | 40.3 | 0.01 | 0 | 0 | never | no | 0 |
| clock +200 ppm | 467 ms | 36.8 | 0.02 | 0 | 0 | never | no | 0 |
| SNR 36 dB | 454 ms | 39.2 | 0.01 | 0 | 0 | never | no | 0 |
| SNR 33 dB | 454 ms | 37.8 | 0.02 | 0 | 0 | never | no | 0 |
| SNR 30 dB | 454 ms | 36.0 | 0.02 | 0 | 0 | never | no | 0 |
| SNR 27 dB | 454 ms | 33.6 | 0.03 | 0 | 0 | never | no | 0 |
| SNR 24 dB | 454 ms | 31.0 | 0.04 | 0 | 0 | never | no | 0 |
| SNR 21 dB | 454 ms | 28.2 | 0.05 | 0 | 0 | never | no | 0 |
| SNR 18 dB | 453 ms | 25.3 | 0.07 | 0 | 0 | never | no | 0 |
| SNR 15 dB | 453 ms | 22.4 | 0.10 | 0 | 0 | never | no | 0 |
| SNR 12 dB | 453 ms | 19.5 | 0.13 | 0 | 0 | never | no | 0 |
| SNR 9 dB | 453 ms | 16.5 | 0.19 | 0 | 0 | never | no | 0 |
| SNR 6 dB | 451 ms | 13.6 | 0.26 | 0 | 0 | never | no | 0 |
| echo 0.57 @ 120 ms | 466 ms | 41.2 | 0.01 | 0 | 0 | never | no | 0 |
| echo -25 dB @ 1.1 s | 466 ms | 41.2 | 0.01 | 0 | 0 | never | no | 0 |
| level +6 dB | 466 ms | 18.6 | 0.08 | 0 | 0 | never | no | 0 |
| level -6 dB | 466 ms | 18.6 | 0.09 | 0 | 0 | never | no | 0 |
| dropout 20 ms | 464 ms | 15.8 | 0.08 | 0.0193 | 0.0120 | 773 ms | no | 0 |
| round trip 1.1 s | 464 ms | 43.0 | 0.01 | 0 | 0 | never | no | 0 |

Lowest signal-to-noise ratio that still carried the payload: **6 dB**.

## V.22bis 2400

600 baud, 4 bits to the symbol.

Arrival phase, clean line: **8/8** of the sample phases carried the payload ([0, 1, 2, 3, 4, 5, 6, 7] worked). Everything below was run at phase 0, the best of the eight.

| case | lock | slicer SNR dB | margin | SER | BER | 1st err | carrier lost | slips |
|---|---|---|---|---|---|---|---|---|
| clean | 175 ms | 28.6 | 0.09 | 0 | 0 | never | no | 0 |
| carrier -7 Hz | never | 11.8 | 0.70 | 0.7209 | 0.3540 | never | no | 0 |
| carrier -3 Hz | never | 11.7 | 0.70 | 0.6223 | 0.3133 | never | no | 0 |
| carrier -1 Hz | 225 ms | 28.6 | 0.10 | 0 | 0 | never | no | 0 |
| carrier +1 Hz | 65 ms | 28.0 | 0.10 | 0 | 0 | never | no | 0 |
| carrier +3 Hz | never | 11.7 | 0.71 | 0.6242 | 0.3258 | never | no | 0 |
| carrier +7 Hz | never | 11.6 | 0.71 | 0.7042 | 0.3588 | never | no | 0 |
| clock -200 ppm | 191 ms | 29.2 | 0.09 | 0 | 0 | never | no | 0 |
| clock -120 ppm | 364 ms | 28.0 | 0.10 | 0 | 0 | never | no | 0 |
| clock -50 ppm | 383 ms | 27.7 | 0.11 | 0 | 0 | never | no | 0 |
| clock +50 ppm | 713 ms | 27.3 | 0.11 | 0 | 0 | never | no | 0 |
| clock +120 ppm | 591 ms | 27.8 | 0.11 | 0 | 0 | never | no | 0 |
| clock +200 ppm | 334 ms | 27.8 | 0.11 | 0.0022 | 0.0008 | 132 ms | no | 0 |
| SNR 36 dB | 175 ms | 28.4 | 0.09 | 0 | 0 | never | no | 0 |
| SNR 33 dB | 196 ms | 28.3 | 0.10 | 0 | 0 | never | no | 0 |
| SNR 30 dB | 175 ms | 28.0 | 0.10 | 0 | 0 | never | no | 0 |
| SNR 27 dB | 175 ms | 27.5 | 0.11 | 0 | 0 | never | no | 0 |
| SNR 24 dB | 196 ms | 26.6 | 0.13 | 0 | 0 | never | no | 0 |
| SNR 21 dB | 180 ms | 25.3 | 0.15 | 0 | 0 | never | no | 0 |
| SNR 18 dB | 180 ms | 23.5 | 0.18 | 0 | 0 | never | no | 0 |
| SNR 15 dB | 180 ms | 21.3 | 0.24 | 0 | 0 | never | no | 0 |
| SNR 12 dB | 180 ms | 18.7 | 0.32 | 0.0030 | 0.0007 | 435 ms | no | 0 |
| SNR 9 dB | 239 ms | 16.0 | 0.44 | 0.0311 | 0.0098 | 207 ms | no | 0 |
| SNR 6 dB | never | 13.4 | 0.60 | 0.3843 | 0.1502 | never | no | 0 |
| echo 0.57 @ 120 ms | 175 ms | 28.6 | 0.09 | 0 | 0 | never | no | 0 |
| echo -25 dB @ 1.1 s | 175 ms | 28.6 | 0.09 | 0 | 0 | never | no | 0 |
| level +6 dB | 401 ms | 18.5 | 0.20 | 0 | 0 | never | no | 0 |
| level -6 dB | 401 ms | 19.1 | 0.21 | 0.0357 | 0.0173 | 837 ms | no | 0 |
| dropout 20 ms | 175 ms | 20.7 | 0.16 | 0.0149 | 0.0092 | 1062 ms | no | 0 |
| round trip 1.1 s | 266 ms | 28.4 | 0.10 | 0 | 0 | never | no | 0 |

Lowest signal-to-noise ratio that still carried the payload: **12 dB**.

## V.32 4800

2400 baud, 2 bits to the symbol.

Arrival phase, clean line: **8/8** of the sample phases carried the payload ([0, 1, 2, 3, 4, 5, 6, 7] worked). Everything below was run at phase 0, the best of the eight.

| case | lock | slicer SNR dB | margin | SER | BER | 1st err | carrier lost | slips |
|---|---|---|---|---|---|---|---|---|
| clean | 17 ms | 36.5 | 0.02 | 0 | 0 | never | no | 0 |
| carrier -7 Hz | 26 ms | 36.4 | 0.02 | 0 | 0 | never | no | 0 |
| carrier -3 Hz | 25 ms | 36.4 | 0.02 | 0 | 0 | never | no | 0 |
| carrier -1 Hz | 25 ms | 36.5 | 0.02 | 0 | 0 | never | no | 0 |
| carrier +1 Hz | 25 ms | 36.5 | 0.02 | 0 | 0 | never | no | 0 |
| carrier +3 Hz | 25 ms | 36.5 | 0.02 | 0 | 0 | never | no | 0 |
| carrier +7 Hz | 25 ms | 36.5 | 0.02 | 0 | 0 | never | no | 0 |
| clock -200 ppm | 26 ms | 36.6 | 0.02 | 0 | 0 | never | no | 0 |
| clock -120 ppm | 27 ms | 36.6 | 0.02 | 0 | 0 | never | no | 0 |
| clock -50 ppm | 27 ms | 36.6 | 0.02 | 0 | 0 | never | no | 0 |
| clock +50 ppm | 24 ms | 36.6 | 0.02 | 0 | 0 | never | no | 0 |
| clock +120 ppm | 24 ms | 36.6 | 0.02 | 0 | 0 | never | no | 0 |
| clock +200 ppm | 20 ms | 36.6 | 0.02 | 0 | 0 | never | no | 0 |
| SNR 36 dB | 17 ms | 33.6 | 0.03 | 0 | 0 | never | no | 0 |
| SNR 33 dB | 17 ms | 32.0 | 0.03 | 0 | 0 | never | no | 0 |
| SNR 30 dB | 17 ms | 29.8 | 0.04 | 0 | 0 | never | no | 0 |
| SNR 27 dB | 17 ms | 27.4 | 0.05 | 0 | 0 | never | no | 0 |
| SNR 24 dB | 17 ms | 24.7 | 0.07 | 0 | 0 | never | no | 0 |
| SNR 21 dB | 17 ms | 21.8 | 0.10 | 0 | 0 | never | no | 0 |
| SNR 18 dB | 17 ms | 19.0 | 0.14 | 0 | 0 | never | no | 0 |
| SNR 15 dB | 17 ms | 16.1 | 0.20 | 0 | 0 | never | no | 0 |
| SNR 12 dB | 17 ms | 13.2 | 0.28 | 0 | 0 | never | no | 0 |
| SNR 9 dB | 17 ms | 10.1 | 0.39 | 0.0122 | 0.0069 | 170 ms | no | 0 |
| SNR 6 dB | 16 ms | 7.7 | 0.52 | 0.1591 | 0.0862 | 69 ms | no | 0 |
| echo 0.57 @ 120 ms | 16 ms | 5.9 | 0.67 | 0.0199 | 0.0112 | 317 ms | no | 0 |
| echo -25 dB @ 1.1 s | 17 ms | 25.1 | 0.07 | 0 | 0 | never | no | 0 |
| level +6 dB | 17 ms | 18.9 | 0.08 | 0 | 0 | never | no | 0 |
| level -6 dB | 17 ms | 18.4 | 0.10 | 0 | 0 | never | no | 0 |
| dropout 20 ms | 17 ms | 15.0 | 0.08 | 0.0107 | 0.0082 | 1093 ms | no | 0 |
| round trip 1.1 s | never | 4.5 | 0.79 | 0.7378 | 0.4898 | never | no | 0 |

Lowest signal-to-noise ratio that still carried the payload: **12 dB**.

## V.32 9600

2400 baud, 4 bits to the symbol.

Arrival phase, clean line: **6/8** of the sample phases carried the payload ([0, 1, 2, 4, 5, 6] worked). Everything below was run at phase 0, the best of the eight.

| case | lock | slicer SNR dB | margin | SER | BER | 1st err | carrier lost | slips |
|---|---|---|---|---|---|---|---|---|
| clean | 127 ms | 31.3 | 0.07 | 0 | 0 | never | no | 0 |
| carrier -7 Hz | never | 11.8 | 0.69 | 0.6618 | 0.3404 | never | no | 0 |
| carrier -3 Hz | 160 ms | 31.3 | 0.07 | 0 | 0 | never | no | 0 |
| carrier -1 Hz | never | 10.1 | 0.74 | 0.9305 | 0.4946 | never | no | 0 |
| carrier +1 Hz | never | 10.0 | 0.75 | 0.9305 | 0.4998 | never | no | 0 |
| carrier +3 Hz | 92 ms | 30.3 | 0.08 | 0.0468 | 0.0242 | 21 ms | no | 0 |
| carrier +7 Hz | 1249 ms | 12.0 | 0.67 | 0.5229 | 0.2617 | 21 ms | no | 0 |
| clock -200 ppm | 155 ms | 30.2 | 0.08 | 0 | 0 | never | no | 0 |
| clock -120 ppm | 167 ms | 31.3 | 0.07 | 0 | 0 | never | no | 0 |
| clock -50 ppm | 163 ms | 31.4 | 0.07 | 0 | 0 | never | no | 0 |
| clock +50 ppm | 162 ms | 31.4 | 0.07 | 0 | 0 | never | no | 0 |
| clock +120 ppm | 161 ms | 31.3 | 0.07 | 0 | 0 | never | no | 0 |
| clock +200 ppm | 159 ms | 31.4 | 0.07 | 0 | 0 | never | no | 0 |
| SNR 36 dB | 130 ms | 30.4 | 0.08 | 0 | 0 | never | no | 0 |
| SNR 33 dB | 127 ms | 29.6 | 0.09 | 0 | 0 | never | no | 0 |
| SNR 30 dB | 130 ms | 28.3 | 0.11 | 0 | 0 | never | no | 0 |
| SNR 27 dB | 130 ms | 26.4 | 0.13 | 0 | 0 | never | no | 0 |
| SNR 24 dB | 127 ms | 24.1 | 0.17 | 0 | 0 | never | no | 0 |
| SNR 21 dB | 127 ms | 21.6 | 0.24 | 0 | 0 | never | no | 0 |
| SNR 18 dB | 102 ms | 18.5 | 0.33 | 0.0024 | 0.0006 | 29 ms | no | 0 |
| SNR 15 dB | 192 ms | 15.7 | 0.45 | 0.0437 | 0.0138 | 77 ms | no | 0 |
| SNR 12 dB | 310 ms | 13.4 | 0.61 | 0.2926 | 0.1003 | 22 ms | no | 0 |
| SNR 9 dB | never | 11.3 | 0.75 | 0.7011 | 0.2955 | never | no | 0 |
| SNR 6 dB | never | 10.1 | 0.77 | 0.8942 | 0.4445 | never | no | 0 |
| echo 0.57 @ 120 ms | never | 9.9 | 0.73 | 0.9389 | 0.5008 | never | no | 0 |
| echo -25 dB @ 1.1 s | 127 ms | 24.3 | 0.17 | 0 | 0 | never | no | 0 |
| level +6 dB | 127 ms | 18.8 | 0.20 | 0.0033 | 0.0009 | 983 ms | no | 0 |
| level -6 dB | 127 ms | 18.1 | 0.22 | 0.0640 | 0.0306 | 983 ms | no | 0 |
| dropout 20 ms | 127 ms | 20.6 | 0.15 | 0.0145 | 0.0075 | 982 ms | no | 0 |
| round trip 1.1 s | 905 ms | 28.6 | 0.09 | 0 | 0 | never | no | 0 |

Lowest signal-to-noise ratio that still carried the payload: **18 dB**.

## V.32bis 7200T

2400 baud, 3 bits to the symbol.

Arrival phase, clean line: **7/8** of the sample phases carried the payload ([0, 1, 2, 4, 5, 6, 7] worked). Everything below was run at phase 0, the best of the eight.

| case | lock | slicer SNR dB | margin | SER | BER | 1st err | carrier lost | slips |
|---|---|---|---|---|---|---|---|---|
| clean | 80 ms | 30.0 | 0.08 | 0 | 0 | never | no | 0 |
| carrier -7 Hz | never | 11.3 | 0.72 | 0.5944 | 0.3354 | never | no | 0 |
| carrier -3 Hz | 90 ms | 30.6 | 0.08 | 0.0310 | 0.0189 | 32 ms | no | 0 |
| carrier -1 Hz | never | 10.0 | 0.75 | 0.8824 | 0.4998 | never | no | 0 |
| carrier +1 Hz | never | 10.1 | 0.72 | 0.8645 | 0.4869 | never | no | 0 |
| carrier +3 Hz | 188 ms | 32.0 | 0.07 | 0.0336 | 0.0178 | 62 ms | no | 0 |
| carrier +7 Hz | never | 11.5 | 0.72 | 0.5895 | 0.3344 | never | no | 0 |
| clock -200 ppm | 173 ms | 32.3 | 0.06 | 0 | 0 | never | no | 0 |
| clock -120 ppm | never | 10.2 | 0.74 | 0.8570 | 0.4884 | never | no | 0 |
| clock -50 ppm | never | 10.1 | 0.74 | 0.8751 | 0.4993 | never | no | 0 |
| clock +50 ppm | 193 ms | 32.3 | 0.06 | 0 | 0 | never | no | 0 |
| clock +120 ppm | 155 ms | 30.1 | 0.08 | 0 | 0 | never | no | 0 |
| clock +200 ppm | 155 ms | 30.1 | 0.08 | 0 | 0 | never | no | 0 |
| SNR 36 dB | 80 ms | 29.4 | 0.09 | 0 | 0 | never | no | 0 |
| SNR 33 dB | 80 ms | 28.7 | 0.10 | 0 | 0 | never | no | 0 |
| SNR 30 dB | 80 ms | 27.7 | 0.11 | 0 | 0 | never | no | 0 |
| SNR 27 dB | 80 ms | 26.1 | 0.14 | 0 | 0 | never | no | 0 |
| SNR 24 dB | 80 ms | 23.9 | 0.18 | 0 | 0 | never | no | 0 |
| SNR 21 dB | 80 ms | 21.5 | 0.24 | 0 | 0 | never | no | 0 |
| SNR 18 dB | 84 ms | 18.7 | 0.32 | 0 | 0 | never | no | 0 |
| SNR 15 dB | 89 ms | 15.9 | 0.45 | 0 | 0 | never | no | 0 |
| SNR 12 dB | 89 ms | 13.4 | 0.60 | 0 | 0 | never | no | 0 |
| SNR 9 dB | 142 ms | 11.5 | 0.74 | 0.4237 | 0.2313 | 32 ms | no | 0 |
| SNR 6 dB | never | 9.5 | 0.77 | 0.8762 | 0.4965 | never | no | 0 |
| echo 0.57 @ 120 ms | never | 9.9 | 0.76 | 0.8678 | 0.4969 | never | no | 0 |
| echo -25 dB @ 1.1 s | 80 ms | 24.1 | 0.17 | 0 | 0 | never | no | 0 |
| level +6 dB | 80 ms | 18.8 | 0.19 | 0 | 0 | never | no | 0 |
| level -6 dB | 98 ms | 18.7 | 0.21 | 0.0322 | 0.0182 | 1022 ms | no | 0 |
| dropout 20 ms | 84 ms | 21.1 | 0.14 | 0.0122 | 0.0070 | 1036 ms | no | 0 |
| round trip 1.1 s | never | 8.4 | 0.78 | 0.8760 | 0.4997 | never | no | 0 |

Lowest signal-to-noise ratio that still carried the payload: **12 dB**.

## V.32 9600T

2400 baud, 4 bits to the symbol.

Arrival phase, clean line: **3/8** of the sample phases carried the payload ([0, 2, 7] worked). Everything below was run at phase 0, the best of the eight.

| case | lock | slicer SNR dB | margin | SER | BER | 1st err | carrier lost | slips |
|---|---|---|---|---|---|---|---|---|
| clean | 111 ms | 32.3 | 0.09 | 0 | 0 | never | no | 0 |
| carrier -7 Hz | never | 14.8 | 0.73 | 0.7245 | 0.3881 | never | no | 0 |
| carrier -3 Hz | 161 ms | 14.8 | 0.72 | 0.6700 | 0.3605 | 31 ms | no | 0 |
| carrier -1 Hz | 230 ms | 32.2 | 0.09 | 0 | 0 | never | no | 0 |
| carrier +1 Hz | 296 ms | 32.1 | 0.09 | 0 | 0 | never | no | 0 |
| carrier +3 Hz | never | 15.0 | 0.71 | 0.9364 | 0.4855 | never | no | 0 |
| carrier +7 Hz | never | 14.6 | 0.74 | 0.7184 | 0.3850 | never | no | 0 |
| clock -200 ppm | 358 ms | 31.0 | 0.10 | 0 | 0 | never | no | 0 |
| clock -120 ppm | never | 14.7 | 0.73 | 0.9312 | 0.4951 | never | no | 0 |
| clock -50 ppm | never | 14.2 | 0.73 | 0.9319 | 0.4973 | never | no | 0 |
| clock +50 ppm | never | 14.7 | 0.73 | 0.9379 | 0.4965 | never | no | 0 |
| clock +120 ppm | never | 14.5 | 0.74 | 0.9291 | 0.4895 | never | no | 0 |
| clock +200 ppm | 233 ms | 32.2 | 0.09 | 0 | 0 | never | no | 0 |
| SNR 36 dB | 111 ms | 31.1 | 0.11 | 0 | 0 | never | no | 0 |
| SNR 33 dB | 111 ms | 30.1 | 0.12 | 0 | 0 | never | no | 0 |
| SNR 30 dB | 111 ms | 28.7 | 0.14 | 0 | 0 | never | no | 0 |
| SNR 27 dB | 105 ms | 26.7 | 0.18 | 0 | 0 | never | no | 0 |
| SNR 24 dB | 106 ms | 24.3 | 0.24 | 0 | 0 | never | no | 0 |
| SNR 21 dB | 106 ms | 21.7 | 0.33 | 0 | 0 | never | no | 0 |
| SNR 18 dB | 106 ms | 18.9 | 0.45 | 0 | 0 | never | no | 0 |
| SNR 15 dB | 133 ms | 16.5 | 0.61 | 0.0033 | 0.0018 | 661 ms | no | 0 |
| SNR 12 dB | 153 ms | 14.3 | 0.75 | 0.4909 | 0.2667 | 33 ms | no | 0 |
| SNR 9 dB | never | 13.3 | 0.78 | 0.9382 | 0.5003 | never | no | 0 |
| SNR 6 dB | never | 12.7 | 0.77 | 0.9352 | 0.5029 | never | no | 0 |
| echo 0.57 @ 120 ms | never | 12.9 | 0.76 | 0.9383 | 0.5011 | never | no | 0 |
| echo -25 dB @ 1.1 s | 111 ms | 24.9 | 0.23 | 0 | 0 | never | no | 0 |
| level +6 dB | 97 ms | 19.2 | 0.26 | 0.0062 | 0.0033 | 1022 ms | no | 0 |
| level -6 dB | 92 ms | 21.4 | 0.23 | 0.0497 | 0.0266 | 1028 ms | no | 0 |
| dropout 20 ms | 111 ms | 23.0 | 0.18 | 0.0135 | 0.0072 | 1008 ms | no | 0 |
| round trip 1.1 s | never | 10.4 | 0.77 | 0.9353 | 0.4963 | never | no | 0 |

Lowest signal-to-noise ratio that still carried the payload: **18 dB**.

## V.32bis 12000T

2400 baud, 5 bits to the symbol.

Arrival phase, clean line: **3/8** of the sample phases carried the payload ([0, 2, 5] worked). Everything below was run at phase 0, the best of the eight.

| case | lock | slicer SNR dB | margin | SER | BER | 1st err | carrier lost | slips |
|---|---|---|---|---|---|---|---|---|
| clean | 126 ms | 27.4 | 0.22 | 0 | 0 | never | no | 0 |
| carrier -7 Hz | never | 16.3 | 0.78 | 0.9690 | 0.4959 | never | no | 0 |
| carrier -3 Hz | never | 16.1 | 0.79 | 0.9688 | 0.5014 | never | no | 0 |
| carrier -1 Hz | never | 16.4 | 0.78 | 0.9708 | 0.4995 | never | no | 0 |
| carrier +1 Hz | 297 ms | 16.1 | 0.79 | 0.8922 | 0.4552 | 45 ms | no | 0 |
| carrier +3 Hz | never | 16.3 | 0.78 | 0.9702 | 0.4985 | never | no | 0 |
| carrier +7 Hz | never | 16.5 | 0.77 | 0.9691 | 0.4973 | never | no | 0 |
| clock -200 ppm | 300 ms | 27.5 | 0.22 | 0 | 0 | never | no | 0 |
| clock -120 ppm | 413 ms | 27.4 | 0.22 | 0 | 0 | never | no | 0 |
| clock -50 ppm | never | 16.1 | 0.78 | 0.9685 | 0.4946 | never | no | 0 |
| clock +50 ppm | 744 ms | 28.7 | 0.20 | 0.0212 | 0.0089 | 70 ms | no | 0 |
| clock +120 ppm | 370 ms | 27.4 | 0.23 | 0 | 0 | never | no | 0 |
| clock +200 ppm | 261 ms | 27.4 | 0.23 | 0 | 0 | never | no | 0 |
| SNR 36 dB | 126 ms | 26.8 | 0.25 | 0 | 0 | never | no | 0 |
| SNR 33 dB | 126 ms | 26.3 | 0.26 | 0 | 0 | never | no | 0 |
| SNR 30 dB | 126 ms | 25.6 | 0.29 | 0 | 0 | never | no | 0 |
| SNR 27 dB | 126 ms | 24.4 | 0.33 | 0 | 0 | never | no | 0 |
| SNR 24 dB | 132 ms | 22.8 | 0.41 | 0 | 0 | never | no | 0 |
| SNR 21 dB | 132 ms | 20.9 | 0.52 | 0 | 0 | never | no | 0 |
| SNR 18 dB | 150 ms | 19.1 | 0.64 | 0.0208 | 0.0103 | 91 ms | no | 0 |
| SNR 15 dB | 295 ms | 17.5 | 0.76 | 0.6292 | 0.2930 | 25 ms | no | 0 |
| SNR 12 dB | never | 16.9 | 0.77 | 0.9646 | 0.4973 | never | no | 0 |
| SNR 9 dB | never | 16.3 | 0.78 | 0.9681 | 0.4980 | never | no | 0 |
| SNR 6 dB | never | 15.7 | 0.78 | 0.9686 | 0.4981 | never | no | 0 |
| echo 0.57 @ 120 ms | never | 16.2 | 0.77 | 0.9662 | 0.4964 | never | no | 0 |
| echo -25 dB @ 1.1 s | 126 ms | 23.4 | 0.39 | 0 | 0 | never | no | 0 |
| level +6 dB | 129 ms | 20.2 | 0.36 | 0.0200 | 0.0095 | 990 ms | no | 0 |
| level -6 dB | 126 ms | 22.8 | 0.35 | 0.0580 | 0.0308 | 994 ms | no | 0 |
| dropout 20 ms | 129 ms | 23.9 | 0.27 | 0.0151 | 0.0081 | 990 ms | no | 0 |
| round trip 1.1 s | never | 12.7 | 0.77 | 0.9670 | 0.4960 | never | no | 0 |

Lowest signal-to-noise ratio that still carried the payload: **21 dB**.

## V.32bis 14400T

2400 baud, 6 bits to the symbol.

Arrival phase, clean line: **2/8** of the sample phases carried the payload ([0, 2] worked). Everything below was run at phase 0, the best of the eight.

| case | lock | slicer SNR dB | margin | SER | BER | 1st err | carrier lost | slips |
|---|---|---|---|---|---|---|---|---|
| clean | 142 ms | 30.7 | 0.22 | 0 | 0 | never | no | 0 |
| carrier -7 Hz | never | 20.1 | 0.78 | 0.9883 | 0.4995 | never | no | 0 |
| carrier -3 Hz | 249 ms | 20.2 | 0.78 | 0.8849 | 0.4424 | 15 ms | no | 0 |
| carrier -1 Hz | 211 ms | 20.2 | 0.77 | 0.8630 | 0.4310 | 33 ms | no | 0 |
| carrier +1 Hz | never | 20.2 | 0.77 | 0.9791 | 0.4957 | never | no | 0 |
| carrier +3 Hz | never | 20.2 | 0.78 | 0.9844 | 0.4945 | never | no | 0 |
| carrier +7 Hz | never | 20.3 | 0.77 | 0.9858 | 0.4907 | never | no | 0 |
| clock -200 ppm | 363 ms | 20.6 | 0.75 | 0.7679 | 0.3853 | 64 ms | no | 0 |
| clock -120 ppm | 579 ms | 30.7 | 0.22 | 0 | 0 | never | no | 0 |
| clock -50 ppm | never | 22.2 | 0.60 | 0.9823 | 0.4959 | never | no | 0 |
| clock +50 ppm | 1189 ms | 23.8 | 0.46 | 0.0100 | 0.0045 | 87 ms | no | 0 |
| clock +120 ppm | 507 ms | 30.6 | 0.22 | 0 | 0 | never | no | 0 |
| clock +200 ppm | 282 ms | 20.5 | 0.75 | 0.7430 | 0.3696 | 190 ms | no | 0 |
| SNR 36 dB | 142 ms | 29.9 | 0.24 | 0 | 0 | never | no | 0 |
| SNR 33 dB | 142 ms | 29.1 | 0.27 | 0 | 0 | never | no | 0 |
| SNR 30 dB | 142 ms | 28.0 | 0.32 | 0 | 0 | never | no | 0 |
| SNR 27 dB | 157 ms | 26.3 | 0.39 | 0 | 0 | never | no | 0 |
| SNR 24 dB | 157 ms | 24.3 | 0.50 | 0 | 0 | never | no | 0 |
| SNR 21 dB | 180 ms | 22.3 | 0.64 | 0.0173 | 0.0072 | 91 ms | no | 0 |
| SNR 18 dB | 232 ms | 20.7 | 0.75 | 0.6082 | 0.2892 | 17 ms | no | 0 |
| SNR 15 dB | never | 20.1 | 0.78 | 0.9864 | 0.5008 | never | no | 0 |
| SNR 12 dB | never | 19.7 | 0.78 | 0.9820 | 0.4965 | never | no | 0 |
| SNR 9 dB | never | 19.1 | 0.80 | 0.9860 | 0.4986 | never | no | 0 |
| SNR 6 dB | never | 18.8 | 0.79 | 0.9832 | 0.4983 | never | no | 0 |
| echo 0.57 @ 120 ms | never | 18.9 | 0.78 | 0.9835 | 0.5001 | never | no | 0 |
| echo -25 dB @ 1.1 s | 142 ms | 24.5 | 0.48 | 0 | 0 | never | no | 0 |
| level +6 dB | 142 ms | 18.3 | 0.76 | 0.1987 | 0.0944 | 976 ms | no | 0 |
| level -6 dB | 150 ms | 25.3 | 0.36 | 0.0775 | 0.0394 | 970 ms | no | 0 |
| dropout 20 ms | 142 ms | 25.6 | 0.33 | 0.0238 | 0.0125 | 977 ms | no | 0 |
| round trip 1.1 s | never | 16.5 | 0.78 | 0.9877 | 0.5011 | never | no | 0 |

Lowest signal-to-noise ratio that still carried the payload: **24 dB**.

## V.29 7200

2400 baud, 3 bits to the symbol.

Arrival phase, clean line: **8/8** of the sample phases carried the payload ([0, 1, 2, 3, 4, 5, 6, 7] worked). Everything below was run at phase 0, the best of the eight.

| case | lock | slicer SNR dB | margin | SER | BER | 1st err | carrier lost | slips |
|---|---|---|---|---|---|---|---|---|
| clean | 268 ms | 30.0 | 0.06 | 0 | 0 | never | no | 0 |
| carrier -7 Hz | 275 ms | 30.0 | 0.06 | 0 | 0 | never | no | 0 |
| carrier -3 Hz | 275 ms | 30.0 | 0.06 | 0 | 0 | never | no | 0 |
| carrier -1 Hz | 275 ms | 30.0 | 0.06 | 0 | 0 | never | no | 0 |
| carrier +1 Hz | 275 ms | 30.0 | 0.06 | 0 | 0 | never | no | 0 |
| carrier +3 Hz | 275 ms | 30.0 | 0.06 | 0 | 0 | never | no | 0 |
| carrier +7 Hz | 275 ms | 29.9 | 0.06 | 0 | 0 | never | no | 0 |
| clock -200 ppm | 268 ms | 30.1 | 0.06 | 0 | 0 | never | no | 0 |
| clock -120 ppm | 268 ms | 30.0 | 0.06 | 0 | 0 | never | no | 0 |
| clock -50 ppm | 268 ms | 30.0 | 0.06 | 0 | 0 | never | no | 0 |
| clock +50 ppm | 268 ms | 30.1 | 0.06 | 0 | 0 | never | no | 0 |
| clock +120 ppm | 268 ms | 30.1 | 0.06 | 0 | 0 | never | no | 0 |
| clock +200 ppm | 268 ms | 30.1 | 0.06 | 0 | 0 | never | no | 0 |
| SNR 36 dB | 268 ms | 29.4 | 0.07 | 0 | 0 | never | no | 0 |
| SNR 33 dB | 268 ms | 28.7 | 0.07 | 0 | 0 | never | no | 0 |
| SNR 30 dB | 268 ms | 27.6 | 0.08 | 0 | 0 | never | no | 0 |
| SNR 27 dB | 268 ms | 26.0 | 0.10 | 0 | 0 | never | no | 0 |
| SNR 24 dB | 804 ms | 15.7 | 0.28 | 0 | 0 | never | no | 0 |
| SNR 21 dB | 268 ms | 21.4 | 0.18 | 0.0021 | 0.0012 | 70 ms | no | 0 |
| SNR 18 dB | 268 ms | 18.6 | 0.24 | 0.0063 | 0.0040 | 30 ms | no | 0 |
| SNR 15 dB | never | 6.4 | 0.89 | 0.5382 | 0.2645 | never | no | 0 |
| SNR 12 dB | never | 6.5 | 0.89 | 0.6781 | 0.3308 | never | no | 0 |
| SNR 9 dB | never | 6.5 | 0.89 | 0.8681 | 0.4929 | never | no | 0 |
| SNR 6 dB | never | 6.3 | 0.88 | 0.8744 | 0.4953 | never | no | 0 |
| echo 0.57 @ 120 ms | never | 7.0 | 0.85 | 0.8684 | 0.4912 | never | no | 0 |
| echo -25 dB @ 1.1 s | 268 ms | 25.6 | 0.10 | 0 | 0 | never | no | 0 |
| level +6 dB | 268 ms | 20.0 | 0.12 | 0 | 0 | never | no | 0 |
| level -6 dB | 268 ms | 16.4 | 0.19 | 0.0500 | 0.0228 | 645 ms | no | 0 |
| dropout 20 ms | 267 ms | 7.8 | 0.72 | 0.3954 | 0.2241 | 644 ms | yes, 12 ms | 0 |
| round trip 1.1 s | 267 ms | 30.0 | 0.06 | 0 | 0 | never | no | 0 |

Lowest signal-to-noise ratio that still carried the payload: **24 dB**.

## V.29 9600

2400 baud, 4 bits to the symbol.

Arrival phase, clean line: **8/8** of the sample phases carried the payload ([0, 1, 2, 3, 4, 5, 6, 7] worked). Everything below was run at phase 0, the best of the eight.

| case | lock | slicer SNR dB | margin | SER | BER | 1st err | carrier lost | slips |
|---|---|---|---|---|---|---|---|---|
| clean | 273 ms | 28.0 | 0.12 | 0 | 0 | never | no | 0 |
| carrier -7 Hz | 281 ms | 28.1 | 0.12 | 0 | 0 | never | no | 0 |
| carrier -3 Hz | 280 ms | 28.1 | 0.12 | 0 | 0 | never | no | 0 |
| carrier -1 Hz | 280 ms | 28.1 | 0.12 | 0 | 0 | never | no | 0 |
| carrier +1 Hz | 280 ms | 28.1 | 0.12 | 0 | 0 | never | no | 0 |
| carrier +3 Hz | 280 ms | 28.1 | 0.12 | 0 | 0 | never | no | 0 |
| carrier +7 Hz | 280 ms | 28.1 | 0.12 | 0 | 0 | never | no | 0 |
| clock -200 ppm | 273 ms | 28.0 | 0.12 | 0 | 0 | never | no | 0 |
| clock -120 ppm | 273 ms | 28.0 | 0.12 | 0 | 0 | never | no | 0 |
| clock -50 ppm | 273 ms | 28.0 | 0.12 | 0 | 0 | never | no | 0 |
| clock +50 ppm | 272 ms | 28.1 | 0.12 | 0 | 0 | never | no | 0 |
| clock +120 ppm | 272 ms | 28.0 | 0.12 | 0 | 0 | never | no | 0 |
| clock +200 ppm | 272 ms | 28.0 | 0.12 | 0 | 0 | never | no | 0 |
| SNR 36 dB | 274 ms | 27.6 | 0.13 | 0 | 0 | never | no | 0 |
| SNR 33 dB | 292 ms | 27.2 | 0.14 | 0 | 0 | never | no | 0 |
| SNR 30 dB | 292 ms | 26.4 | 0.15 | 0 | 0 | never | no | 0 |
| SNR 27 dB | 292 ms | 25.1 | 0.18 | 0 | 0 | never | no | 0 |
| SNR 24 dB | 267 ms | 23.2 | 0.23 | 0 | 0 | never | no | 0 |
| SNR 21 dB | 308 ms | 21.1 | 0.28 | 0 | 0 | never | no | 0 |
| SNR 18 dB | 1300 ms | 11.7 | 0.80 | 0.0279 | 0.0084 | 60 ms | no | 0 |
| SNR 15 dB | never | 10.2 | 1.01 | 0.6797 | 0.2822 | never | no | 0 |
| SNR 12 dB | never | 10.4 | 0.97 | 0.8185 | 0.3591 | never | no | 0 |
| SNR 9 dB | never | 9.4 | 0.88 | 0.9359 | 0.4901 | never | no | 0 |
| SNR 6 dB | never | 9.4 | 0.89 | 0.9378 | 0.4930 | never | no | 0 |
| echo 0.57 @ 120 ms | never | 9.0 | 0.86 | 0.9409 | 0.4970 | never | no | 0 |
| echo -25 dB @ 1.1 s | 273 ms | 24.6 | 0.18 | 0 | 0 | never | no | 0 |
| level +6 dB | 267 ms | 20.9 | 0.19 | 0.0049 | 0.0016 | 645 ms | no | 0 |
| level -6 dB | 267 ms | 22.3 | 0.18 | 0.0313 | 0.0143 | 644 ms | no | 0 |
| dropout 20 ms | 267 ms | 11.5 | 0.81 | 0.4200 | 0.2238 | 644 ms | yes, 13 ms | 0 |
| round trip 1.1 s | 272 ms | 28.0 | 0.12 | 0 | 0 | never | no | 0 |

Lowest signal-to-noise ratio that still carried the payload: **21 dB**.

## V.27ter 2400

1200 baud, 2 bits to the symbol.

Arrival phase, clean line: **8/8** of the sample phases carried the payload ([0, 1, 2, 3, 4, 5, 6, 7] worked). Everything below was run at phase 0, the best of the eight.

| case | lock | slicer SNR dB | margin | SER | BER | 1st err | carrier lost | slips |
|---|---|---|---|---|---|---|---|---|
| clean | 968 ms | 50.4 | 0.01 | 0 | 0 | never | no | 0 |
| carrier -7 Hz | 976 ms | 50.4 | 0.01 | 0 | 0 | never | no | 0 |
| carrier -3 Hz | 976 ms | 50.4 | 0.01 | 0 | 0 | never | no | 0 |
| carrier -1 Hz | 976 ms | 50.4 | 0.01 | 0 | 0 | never | no | 0 |
| carrier +1 Hz | 976 ms | 50.4 | 0.01 | 0 | 0 | never | no | 0 |
| carrier +3 Hz | 976 ms | 50.4 | 0.01 | 0 | 0 | never | no | 0 |
| carrier +7 Hz | 976 ms | 50.4 | 0.01 | 0 | 0 | never | no | 0 |
| clock -200 ppm | 968 ms | 50.7 | 0.01 | 0 | 0 | never | no | 0 |
| clock -120 ppm | 968 ms | 50.8 | 0.01 | 0 | 0 | never | no | 0 |
| clock -50 ppm | 968 ms | 50.8 | 0.01 | 0 | 0 | never | no | 0 |
| clock +50 ppm | 968 ms | 50.7 | 0.01 | 0 | 0 | never | no | 0 |
| clock +120 ppm | 968 ms | 50.7 | 0.01 | 0 | 0 | never | no | 0 |
| clock +200 ppm | 968 ms | 50.7 | 0.01 | 0 | 0 | never | no | 0 |
| SNR 36 dB | 968 ms | 39.6 | 0.02 | 0 | 0 | never | no | 0 |
| SNR 33 dB | 968 ms | 36.7 | 0.03 | 0 | 0 | never | no | 0 |
| SNR 30 dB | 968 ms | 33.9 | 0.05 | 0 | 0 | never | no | 0 |
| SNR 27 dB | 968 ms | 30.9 | 0.07 | 0 | 0 | never | no | 0 |
| SNR 24 dB | 968 ms | 27.9 | 0.09 | 0 | 0 | never | no | 0 |
| SNR 21 dB | 968 ms | 25.0 | 0.13 | 0 | 0 | never | no | 0 |
| SNR 18 dB | 968 ms | 22.0 | 0.18 | 0 | 0 | never | no | 0 |
| SNR 15 dB | 968 ms | 19.0 | 0.26 | 0 | 0 | never | no | 0 |
| SNR 12 dB | 969 ms | 16.0 | 0.37 | 0.0015 | 0.0011 | 1084 ms | no | 0 |
| SNR 9 dB | 1128 ms | 13.2 | 0.51 | 0.0497 | 0.0321 | 106 ms | no | 0 |
| SNR 6 dB | never | 10.4 | 0.72 | 0.7344 | 0.4854 | never | no | 0 |
| echo 0.57 @ 120 ms | never | 7.8 | 0.92 | 0.7357 | 0.4867 | never | no | 0 |
| echo -25 dB @ 1.1 s | 968 ms | 24.8 | 0.15 | 0 | 0 | never | no | 0 |
| level +6 dB | 968 ms | 20.3 | 0.12 | 0 | 0 | never | no | 0 |
| level -6 dB | 968 ms | 19.9 | 0.13 | 0 | 0 | never | no | 0 |
| dropout 20 ms | never | 11.8 | 0.51 | 0.7318 | 0.4873 | never | yes, 11 ms | 0 |
| round trip 1.1 s | 968 ms | 50.4 | 0.01 | 0 | 0 | never | no | 0 |

Lowest signal-to-noise ratio that still carried the payload: **15 dB**.

## V.27ter 4800

1600 baud, 3 bits to the symbol.

Arrival phase, clean line: **8/8** of the sample phases carried the payload ([0, 1, 2, 3, 4, 5, 6, 7] worked). Everything below was run at phase 0, the best of the eight.

| case | lock | slicer SNR dB | margin | SER | BER | 1st err | carrier lost | slips |
|---|---|---|---|---|---|---|---|---|
| clean | 727 ms | 49.3 | 0.01 | 0 | 0 | never | no | 0 |
| carrier -7 Hz | 735 ms | 49.3 | 0.01 | 0 | 0 | never | no | 0 |
| carrier -3 Hz | 735 ms | 49.3 | 0.01 | 0 | 0 | never | no | 0 |
| carrier -1 Hz | 735 ms | 49.3 | 0.01 | 0 | 0 | never | no | 0 |
| carrier +1 Hz | 735 ms | 49.3 | 0.01 | 0 | 0 | never | no | 0 |
| carrier +3 Hz | 735 ms | 49.3 | 0.01 | 0 | 0 | never | no | 0 |
| carrier +7 Hz | 735 ms | 49.3 | 0.01 | 0 | 0 | never | no | 0 |
| clock -200 ppm | 727 ms | 49.1 | 0.01 | 0 | 0 | never | no | 0 |
| clock -120 ppm | 727 ms | 49.1 | 0.01 | 0 | 0 | never | no | 0 |
| clock -50 ppm | 727 ms | 49.3 | 0.01 | 0 | 0 | never | no | 0 |
| clock +50 ppm | 727 ms | 49.3 | 0.01 | 0 | 0 | never | no | 0 |
| clock +120 ppm | 727 ms | 48.9 | 0.01 | 0 | 0 | never | no | 0 |
| clock +200 ppm | 727 ms | 48.8 | 0.01 | 0 | 0 | never | no | 0 |
| SNR 36 dB | 727 ms | 38.5 | 0.03 | 0 | 0 | never | no | 0 |
| SNR 33 dB | 727 ms | 35.7 | 0.04 | 0 | 0 | never | no | 0 |
| SNR 30 dB | 727 ms | 32.7 | 0.05 | 0 | 0 | never | no | 0 |
| SNR 27 dB | 727 ms | 29.8 | 0.08 | 0 | 0 | never | no | 0 |
| SNR 24 dB | 727 ms | 26.8 | 0.11 | 0 | 0 | never | no | 0 |
| SNR 21 dB | 727 ms | 23.8 | 0.15 | 0 | 0 | never | no | 0 |
| SNR 18 dB | 727 ms | 20.8 | 0.21 | 0 | 0 | never | no | 0 |
| SNR 15 dB | 727 ms | 17.9 | 0.30 | 0 | 0 | never | no | 0 |
| SNR 12 dB | 727 ms | 14.9 | 0.42 | 0.0106 | 0.0045 | 307 ms | no | 0 |
| SNR 9 dB | 1343 ms | 12.2 | 0.57 | 0.1346 | 0.0708 | 46 ms | no | 0 |
| SNR 6 dB | never | 9.8 | 0.77 | 0.8644 | 0.4918 | never | no | 0 |
| echo 0.57 @ 120 ms | never | 7.5 | 1.00 | 0.8772 | 0.4983 | never | no | 0 |
| echo -25 dB @ 1.1 s | 727 ms | 24.7 | 0.15 | 0 | 0 | never | no | 0 |
| level +6 dB | 727 ms | 20.2 | 0.12 | 0 | 0 | never | no | 0 |
| level -6 dB | 727 ms | 20.0 | 0.13 | 0 | 0 | never | no | 0 |
| dropout 20 ms | never | 11.9 | 0.49 | 0.8624 | 0.4918 | never | yes, 11 ms | 0 |
| round trip 1.1 s | 727 ms | 49.3 | 0.01 | 0 | 0 | never | no | 0 |

Lowest signal-to-noise ratio that still carried the payload: **15 dB**.

## Where it stands, worst first

Ranked by how much of the sweep the mode did not survive: two points          for every arrival phase that carried nothing, one for every impairment          case that failed.

| mode | arrival phases | SNR floor | failed | what failed |
|---|---|---|---|---|
| V.32bis 14400T | 2/8 | 24 dB | 15 | carrier -7 Hz (BER 0.499), carrier -3 Hz (BER 0.442), carrier -1 Hz (BER 0.431), carrier +1 Hz (BER 0.496), carrier +3 Hz (BER 0.495), carrier +7 Hz (BER 0.491), clock -200 ppm (BER 0.385), clock -50 ppm (BER 0.496), clock +50 ppm (BER 0.005), clock +200 ppm (BER 0.370), echo 0.57 @ 120 ms (BER 0.500), level +6 dB (BER 0.094), level -6 dB (BER 0.039), dropout 20 ms (BER 0.013), round trip 1.1 s (BER 0.501) |
| V.32 9600T | 3/8 | 18 dB | 13 | carrier -7 Hz (BER 0.388), carrier -3 Hz (BER 0.361), carrier +3 Hz (BER 0.485), carrier +7 Hz (BER 0.385), clock -120 ppm (BER 0.495), clock -50 ppm (BER 0.497), clock +50 ppm (BER 0.497), clock +120 ppm (BER 0.489), echo 0.57 @ 120 ms (BER 0.501), level +6 dB (BER 0.003), level -6 dB (BER 0.027), dropout 20 ms (BER 0.007), round trip 1.1 s (BER 0.496) |
| V.32bis 12000T | 3/8 | 21 dB | 13 | carrier -7 Hz (BER 0.496), carrier -3 Hz (BER 0.501), carrier -1 Hz (BER 0.500), carrier +1 Hz (BER 0.455), carrier +3 Hz (BER 0.498), carrier +7 Hz (BER 0.497), clock -50 ppm (BER 0.495), clock +50 ppm (BER 0.009), echo 0.57 @ 120 ms (BER 0.496), level +6 dB (BER 0.010), level -6 dB (BER 0.031), dropout 20 ms (BER 0.008), round trip 1.1 s (BER 0.496) |
| V.32bis 7200T | 7/8 | 12 dB | 12 | carrier -7 Hz (BER 0.335), carrier -3 Hz (BER 0.019), carrier -1 Hz (BER 0.500), carrier +1 Hz (BER 0.487), carrier +3 Hz (BER 0.018), carrier +7 Hz (BER 0.334), clock -120 ppm (BER 0.488), clock -50 ppm (BER 0.499), echo 0.57 @ 120 ms (BER 0.497), level -6 dB (BER 0.018), dropout 20 ms (BER 0.007), round trip 1.1 s (BER 0.500) |
| V.32 9600 | 6/8 | 18 dB | 8 | carrier -7 Hz (BER 0.340), carrier -1 Hz (BER 0.495), carrier +1 Hz (BER 0.500), carrier +3 Hz (BER 0.024), carrier +7 Hz (BER 0.262), echo 0.57 @ 120 ms (BER 0.501), level -6 dB (BER 0.031), dropout 20 ms (BER 0.008) |
| V.22 1200 | 7/8 | 6 dB | 4 | carrier -7 Hz (BER 0.109), carrier +7 Hz (BER 0.108), clock +120 ppm (BER 0.469), dropout 20 ms (BER 0.012) |
| V.22bis 2400 | 8/8 | 12 dB | 6 | carrier -7 Hz (BER 0.354), carrier -3 Hz (BER 0.313), carrier +3 Hz (BER 0.326), carrier +7 Hz (BER 0.359), level -6 dB (BER 0.017), dropout 20 ms (BER 0.009) |
| V.29 9600 | 8/8 | 21 dB | 4 | echo 0.57 @ 120 ms (BER 0.497), level +6 dB (BER 0.002), level -6 dB (BER 0.014), dropout 20 ms (BER 0.224) |
| V.22bis 1200 | 8/8 | 6 dB | 3 | carrier -7 Hz (BER 0.061), carrier +7 Hz (BER 0.065), dropout 20 ms (BER 0.012) |
| V.32 4800 | 8/8 | 12 dB | 3 | echo 0.57 @ 120 ms (BER 0.011), dropout 20 ms (BER 0.008), round trip 1.1 s (BER 0.490) |
| V.29 7200 | 8/8 | 24 dB | 3 | echo 0.57 @ 120 ms (BER 0.491), level -6 dB (BER 0.023), dropout 20 ms (BER 0.224) |
| V.27ter 2400 | 8/8 | 15 dB | 2 | echo 0.57 @ 120 ms (BER 0.487), dropout 20 ms (BER 0.487) |
| V.27ter 4800 | 8/8 | 15 dB | 2 | echo 0.57 @ 120 ms (BER 0.498), dropout 20 ms (BER 0.492) |
| Bell 103 300 | 8/8 | 6 dB | 1 | dropout 20 ms (BER 0.016) |
| V.21 300 | 8/8 | 6 dB | 1 | dropout 20 ms (BER 0.001) |

## Two of those, pinned down

### What does a silence before the carrier do to V.32?

Every V.32 row's `round trip 1.1 s` failure is this. The far end's signal is unchanged; the only difference is how long the receiver listened to nothing first. V.22 bis, which is structurally the same receiver, is run beside it as the control.

It is not a clean threshold — the silence and the arrival phase interact, which is the §3.1 fault of `evidence.md` showing through — but the effect is unmistakable and it is the only impairment in this whole sweep that breaks V.32 at 4800, which is otherwise eight phases out of eight on everything.

| silence before the carrier | V.32 4800 phases held | V.32 4800 BER at phase 0 | V.32 4800 lock | V.22bis 2400 phases held |
|---|---|---|---|---|
| 0 ms | 8/8 | 0 | 17 ms | 8/8 |
| 50 ms | 8/8 | 0 | 17 ms | 8/8 |
| 100 ms | 8/8 | 0 | 17 ms | 8/8 |
| 150 ms | 8/8 | 0 | 17 ms | 8/8 |
| 200 ms | 4/8 | 0 | 27 ms | 8/8 |
| 250 ms | 6/8 | 0 | 269 ms | 8/8 |
| 300 ms | 0/8 | 0.4913 | never | 8/8 |
| 400 ms | 5/8 | 0.4913 | never | 8/8 |
| 600 ms | 5/8 | 0.4901 | never | 8/8 |
| 1100 ms | 5/8 | 0.4898 | never | 8/8 |


### Is a carrier or clock failure the offset, or the arrival phase again?

Above 9600 the clean line already only carries the payload at some of the eight arrival phases, so a single-phase row cannot tell an impairment the receiver genuinely cannot take from the same lottery re-rolled. Each cell below is how many of the eight phases carried the payload with that impairment applied, so a column that falls to 0/8 everywhere is the impairment and one that merely wobbles is the phase.

| mode | clean | -7 Hz | -3 Hz | -1 Hz | +1 Hz | +3 Hz | +7 Hz | -200 ppm | +200 ppm |
|---|---|---|---|---|---|---|---|---|---|
| V.22 1200 | 7/8 | 0/8 | 8/8 | 8/8 | 8/8 | 8/8 | 0/8 | 8/8 | 8/8 |
| V.22bis 2400 | 8/8 | 0/8 | 0/8 | 3/8 | 5/8 | 0/8 | 0/8 | 8/8 | 8/8 |
| V.32 4800 | 8/8 | 8/8 | 8/8 | 8/8 | 8/8 | 8/8 | 8/8 | 8/8 | 8/8 |
| V.32 9600 | 6/8 | 0/8 | 6/8 | 4/8 | 4/8 | 3/8 | 0/8 | 5/8 | 4/8 |
| V.32bis 7200T | 7/8 | 0/8 | 0/8 | 3/8 | 4/8 | 0/8 | 0/8 | 5/8 | 3/8 |
| V.32 9600T | 3/8 | 0/8 | 0/8 | 7/8 | 7/8 | 0/8 | 0/8 | 5/8 | 5/8 |
| V.32bis 12000T | 3/8 | 0/8 | 0/8 | 0/8 | 0/8 | 0/8 | 0/8 | 7/8 | 7/8 |
| V.32bis 14400T | 2/8 | 0/8 | 0/8 | 0/8 | 0/8 | 0/8 | 0/8 | 0/8 | 0/8 |
| V.29 7200 | 8/8 | 8/8 | 8/8 | 8/8 | 8/8 | 8/8 | 8/8 | 8/8 | 8/8 |
| V.29 9600 | 8/8 | 8/8 | 8/8 | 8/8 | 8/8 | 8/8 | 8/8 | 8/8 | 8/8 |
| V.27ter 2400 | 8/8 | 8/8 | 8/8 | 8/8 | 8/8 | 8/8 | 8/8 | 8/8 | 8/8 |
| V.27ter 4800 | 8/8 | 8/8 | 8/8 | 8/8 | 8/8 | 8/8 | 8/8 | 8/8 | 8/8 |

### How long a hole can each mode take?

`dropout 20 ms` is the one case every mode in the sweep failed. It is not one failure but two. Below about 10 ms nothing declares the carrier gone and the cost is proportional to the hole. At 20 ms the two fax modes' carrier detectors correctly drop — and dropping it is what ends the burst, because a half-duplex receiver has one training sequence in front of it and no way back to it. V.22 bis and V.32 never drop the carrier at all, at any length of hole, so nothing above them is ever told.

| mode | 2 ms | 5 ms | 10 ms | 20 ms | 50 ms |
|---|---|---|---|---|---|
| V.27ter 4800 | BER 0.0011, carrier held | BER 0.0022, carrier held | BER 0.0048, carrier held | BER 0.4947, carrier lost | BER 0.4769, carrier lost |
| V.29 9600 | BER 0.0025, carrier held | BER 0.0033, carrier held | BER 0.0053, carrier held | BER 0.2195, carrier lost | BER 0.2176, carrier lost |
| V.22bis 2400 | BER 0.0008, carrier held | BER 0.0033, carrier held | BER 0.0046, carrier held | BER 0.0073, carrier held | BER 0.0177, carrier held |
| V.32 4800 | BER 0.0013, carrier held | BER 0.0031, carrier held | BER 0.0046, carrier held | BER 0.0077, carrier held | BER 0.0161, carrier held |

