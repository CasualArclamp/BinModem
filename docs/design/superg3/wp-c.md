# WP-C: the primary channel receiver's references

Branch `wp/c` from `superg3`; one file, `crates/datapump/src/v34/receiver.rs`, and its tests.

## What was built

- **`Reference::PpThenTrnAt(Size)`**, half-duplex phase 3 (12.3.2): S, S-bar, PP, then TRN at four or sixteen points (INFOh bit 30), of any length INFOh gives, none included. The solve is over PP alone, and TRN is then decided at `size` from its first symbol. A slip in PP falls back to the old second try over TRN's symbols 256 to 512, which needs TRN of 512 symbols or more.
- **`Reference::Pp`**, the page resync (12.5.2). The solve is over PP alone, from nothing: no old taps, phase or timing. There is no second try. The first `Heard::Symbol` after `Heard::Trained` is B1's first symbol.
  - It leaves the slicer as it was. Symbols wait for the next half-symbol sample, so `set_grid` on `Trained` is in time.
  - A failed attempt says `Untrained` and hunts again at once, through the kept samples from just after the S-bar it tried.
- `PpThenTrn` (duplex) and `Trn(Size)` are unchanged, bit for bit.

## Departures, and why
- **`PpThenTrn` did not become `PpThenTrn(Size)`.** Five callers outside this package name it bare: `training.rs:1353`, `v90/digital.rs:926`, `tests/v34_capture.rs:136`, `tests/v34_vector.rs:151` and `tests/v90_vector.rs:159`. So the sized form is a new variant. It is not a drop-in for duplex, because the window differs.
- **Half-duplex phase 3 solves over PP alone, not PP plus 64 TRN symbols.** TRN's rows sit at the window's end, where a clock offset has moved the timing most. At 50 ppm, adding them cost 0.2-0.8 dB at four points and 1.2-1.7 dB at sixteen; with the clocks together all were within 0.1 dB. PP alone makes four and sixteen points train alike, and at least as well as duplex's four.
- **The re-hunt was not asked for.** An answer modem's 1800 Hz guard tone, alone for a few ms where its control channel starts and stops (dpsk.rs's is), passed for S at 9 of the 11 carriers and for S-bar at 5. The duplex hunt was left alone, because a band-edge test like `dsp::qam::hunt`'s would reject real S at 3429 over a phone line.
- **Old taps are not used.** They would need the timing (to a fraction of a symbol) and the phase found first. PP alone came within ±0.55 dB of phase 3 at every rate, and 47 dB on a receiver never trained.

## Measured (tests in receiver.rs)
- **Phase 3, 50 ppm, 45 dB:** sixteen points trained to 42.9-43.8 dB, four to 42.9-43.8 and duplex to 42.7-43.0. Tracked through TRN, sixteen points came out up to 0.3 dB under four.
- **127 × 35 ms of TRN** (15 240 symbols, 16 points, 114 ppm): every symbol descrambles to ones, 46 dB at the end. TRN of 0 also trains.
- **Page after 3 s of control channel,** fraction-of-a-symbol shifts, carrier turned, 50 dB: every symbol rate at its top rate decodes exactly from B1. The resync reached 50.4-51.8 dB.
- **Four pages at 33 dB** of noise, 25-70 ppm, a 20 ms slip in each gap: resync 37-39 dB, 19 200-28 800 bit/s exact. At 30 dB: 34-36 dB, still exact.

## For package E (recipient)
- **Phase 3.**
  1. After INFOh, build `Receiver::new(Band::new(rate, high), fs)`, then call `hunt()`.
  2. On `Reversal { at }`, call `train(Reference::PpThenTrnAt(size), far, at)`; `far` is the source's scrambler `Mode`.
  3. After `Trained`, the next Symbol is TRN's first, or TRN's 512th after a slip. So judge TRN's end by `halves()` reaching `at + 2 * (16 + 288 + trn)`, then call `idle()`.
- **Each page.**
  1. Call `hunt()` once this end has gone quiet (F.3.2.2); it is fine while the source's control channel still runs.
  2. On `Reversal`, call `train(Reference::Pp, far, at)`.
  3. On `Trained`, build `Decoder::new(params)` and call `set_grid(decoder.grid_scale(), decoder.extent())`, or call `set_grid` any time earlier.
  4. Feed every Symbol from then on. The first `framing.n` bits are B1's ones.
  5. On `Untrained`, do nothing: calling `hunt()` would forget an S already heard.
  6. At the page's end, call `idle()`. `resume()` is not used.
- **Slips.** A slip in S-bar or PP loses the page: `Untrained`, and no S until the next one. A slip in data is followed (`slips()`), but the Decoder loses its frames, so use `data::Acquirer` as `training.rs` does.
- **Precoding.** The equaliser is a full linear one, so MPh should ask for no precoding.
- **For package D.** Ask INFOh for TRN of 7 × 35 ms or more if phase 3's slip retry is to have TRN to search.
