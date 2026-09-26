# WP-F: the V.34 half-duplex control channel modem

`crates/datapump/src/v34/control.rs` and `control/{transmitter,receiver,watch,tests}.rs`
(10.2.3.3, 10.2.4): the 600 baud QAM channel every T.30 frame of a V.34 fax goes over, both
ways at once, 1200 Hz from the call modem and 2400 Hz with the 1800 Hz guard tone from the
answer modem. The modem only; the procedures of 12.4, 12.6 and 12.8 are package G's.

## Public API (`v34::control`)

- `Modem { transmitter, receiver }`: `new(side, fs)`, `step(sample) -> sample`.
- `Transmitter`: `queue(Segment)`, `clear`, `stop`, `send_bits`, `pending_bits`, `set_rate`,
  `sending`/`on_air() -> Option<Kind>`, `sent() -> Option<Sent>` (`Began`/`Ended { kind, at }`),
  `now`, `next_sample`. `Segment`: `Silence(n)`, `Pph(Reading)`, `Alt { at_least }`, `Ac`,
  `Sh(n)`, `ShBar(n)`, `Bits(Vec)`, `Repeat(Vec)` (an MPh), `E`, `Ones(n)`, `Data`.
- `Receiver`: `feed(sample)`, `heard() -> Option<Heard>`, `hearing() -> Hearing`,
  `hearing_since`/`hearing_for`, `carrier`, `level`, `phase`, `is_locked`, `e_seen`,
  `set_rate`, `reference`, `take_sync_bits`, `take_bits`, `stop`, `reset`, `snr_db`,
  `trained_snr_db`, `slips`, `drift_ppm`, `offset_hz`, `now`. `Heard`: `Carrier { on, at }`,
  `Tone`, `Ac`, `Sh`, `Reversal`, `Pph { reading, began, ended }`, `Trained { on, snr_db }`,
  `Untrained`, `E { at }`, `Lost { at }`. Also `Rate`, `Reading`, `pph`, `sh`, `sh_bar`,
  `ac` and clause 12's lengths (`PPH_SYMBOLS`, `SH_SYMBOLS`, `ALT_LEAST`, `AC_SECONDS`...).

## How the unstated details were settled

- PPh is `Reading::WithI` by default (plan 8.1); the receiver looks for both and `reference()`
  says which came. ALT starts with a 0 and Z = 0; data after E stays differentially encoded
  (plan 8.4). The turn-off's 4T of ones go at the data rate.
- Every signal leaves at unit mean power, the nominal level, whatever its constellation
  (10.2.4 gives no relative scaling; digest F-V34 #5). The pulse reaches 8 symbols each side,
  Hann-tapered (dpsk.rs: 6), so under 1e-6 of this end's power lands in the far band. The
  guard tone ramps over 8 ms, three symbols before the first to one after the last, not
  switched as in dpsk.rs: its switched edge cost the answer modem's own training 14 dB.
- Receiver: a 65 dB Kaiser band-pass about the far carrier (flat to 480 Hz, stopping by 590)
  ahead of `dsp::qam::Core` with `Options::fixed()`. A watch classes the far signal by its
  self-correlation at lags 1, 2, 4 and 8 symbols before anything is trained. PPh (or Sh +
  S-bar-h) is found by correlation once all 32 symbols could be in; the symbols after it are
  decided on its references and handed up at once; the core is then trained by least squares
  on 80 symbols, the 32 known plus the decided ALT and MPh (PPh alone is 32 rows against 31
  taps); when 2400 bit/s follows E the window ends at E. Carrier on 54 dB under nominal, off
  at 59 (V.32's levels), 20 ms envelope; `Lost` after 0.1 s without it, 0.25 s with the core
  lost, or a tone, AC or Sh heard mid-data.

## Measured, and what package G needs to know

Two-wire line (100 ms each way, the answer end 20 dB down at the call end, each end's own signal
10 dB down in its receiver, guard tone, 50 ppm): start-up and data both ways at 2400 bit/s from
30 to 18 dB voice-band SNR and at 1200 at 10 dB, trained within 3 dB of what the noise in 600 Hz
allows; a 20 ms slip costs exactly the bits it took, no resync; 7 Hz offsets followed; Figure 13 met.

- `Alt` (past `at_least`), `Ac`, `Repeat` and `Data` run until something is queued behind them;
  `Repeat` finishes its repetition (12.4.1.3), `Data` drains `send_bits`; idle `Data` sends ones.
- `Heard::at` is an input sample (`Receiver::now`), the filter delay removed; `Sent::at` an
  output sample. `Heard::Pph` comes about a symbol after PPh ends plus 18 ms; `Trained` about
  80 symbols after PPh began, but bits flow from ALT's first symbol. Act on `Heard::Reversal`
  for 12.6, not `Heard::Sh`. A PPh heard in data retrains by itself (12.8); `hearing_for()`
  measures 12.8.2's 100 ms of AC; `reset()` forgets the taps for a start from nothing.
- The receiver's `set_rate` is the rate this end asked for in its own MPh (bit 27), the
  transmitter's the far end's; ALT, MPh and E are always 1200 bit/s; both survive a resync.
  MPh is not decoded here: feed `take_sync_bits()` to package AB's codec; data is `take_bits()`.
