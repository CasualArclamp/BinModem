# WP-E: the primary channel, both ends

Branch `wp/e` from `superg3`; one new file, `crates/datapump/src/v34/primary.rs`, its `pub mod` line, and this note.

## What was built

- **`Channel`**: what INFOh chose (Table 22) -- `band`, `pre_emphasis`, `reduction`, `trn_size`, `trn_steps` -- with `trn_symbols()` and `turn_off_symbols()`. **`DataMode`**: what MPh settled -- `rate`, `code`, `nonlinear`, `expanded`, `precoding` -- and `params(symbol_rate, source_mode) -> Option<Params>` (no auxiliary channel; None if Table 8 has no such rate).
- **`Source::new(channel, own_mode, fs)`**: one `qam::Transmitter` for the call, zero samples between bursts so it can be summed with the control channel's at all times. `phase3()`: 70 ms, S 128T, S-bar 16T, PP, TRN at INFOh's size and length (12.3.1). `page(data) -> bool`: 70 ms, S, S-bar, PP, B1, then bits from `push_bits` (ones while there are none), `pending_bits()`. `end_page()`: circuit 105 off -- what is queued goes, then the turn-off, then the pulse flushes. `sending() -> Sending` {Idle, Silence, S, SBar, Pp, Trn, B1, Data, TurningOff, Flushing}; `is_sending()`; `next_sample()`.
- **`Recipient::new(channel, far_mode, fs)`**: `expect_phase3()` after INFOh; `expect_page(data) -> bool` once this end is quiet; `stop()`; `feed(sample)`; `event() -> Option<Event>`; `take_bits()`. `Event`: `Trained { snr_db }`, `Phase3Over { well }`, `PageStarted { b1_errors }` (109 on), `PageEnded` (109 off). Readings: `state()`, `carrier()`, `level_db()`, `snr_db()`, `trained_snr_db()`, `slips()`, `b1_errors()`, `trn_share()`, `last_point()`, `drift_ppm()`.
- Phase 3 is sequenced as wp-c.md says: hunt, `PpThenTrnAt(size)`, TRN read with `signals::Reader` until `halves()` reaches `at + 2(16 + 288 + trn)`, then `idle()`. A page: hunt, `Pp` with the grid set at the reversal, `Decoder` from B1's first symbol, B1's errors counted, `Acquirer` when `slips()` changes or the path cost stays over 1.0 for 1000 symbols (training.rs's rule).

## Decisions

- **Turn-off (F-V34 item 14): 35 ms of ones made up to whole mapping frames**, begun with the first frame after the page's last bit -- 88, 96, 104, 112, 112, 120 symbols, 35.0 to 37.3 ms. A mapping frame is what the encoder takes bits in; a data frame would be 40 ms at four rates of six, more than 12.5.3.1 says; and what the recipient needs of the ones is its decoder's 40 4D symbols of traceback carried past the last bit, which 35 ms does at every rate (80 symbols is 33.3 ms at 2400).
- **"TRN not satisfactorily received" (12.3.3)**: TRN descrambling to ones fewer than 99 times in 100 once the descrambler has filled; no S-bar in 2000 ms from `expect_phase3()`; or PP that never trains inside them (an `Untrained` hunts again until the deadline). TRN of no length is judged by PP alone. Phase 3's taps are not kept -- every page trains again on its PP -- so TRN is a line check, not the training.
- **Page end (6.6.2, 12.5.3.2)**: a level detector on the raw samples, since the receiver's `level()` is a quarter of a second behind. The thresholds are relative -- nothing here knows dBm on a VoIP line -- against the page's own level learned from training on: off 12 dB under it for 20 ms, on again 10 dB under (the 2 dB of hysteresis), the level followed over 1 ms and the reference over 100 ms, never dragged down by a dip. Measured 21.7 to 23.1 ms after the last symbol's centre at every rate; a carrier that dips 6 or 10 dB for 300 ms is not an end. A page that never trains never ends: G should rely on the control channel's Sh for that turn.
- **The recipient's MPh must ask for neither precoding nor non-linear encoding.** The receiver's slicer is linear: with 9.7's bent outer points a page read as lost from B1 on (30 dB where 43 was trained). Duplex's own MP sends non-linear 0 too (`training.rs:1523`). The source encodes either if the far recipient asks.
- **12.6.1.1's 70 ms of silence is what the detector relies on.** The first harness ran the control channel straight after a page, and at 2400 baud its 2400 Hz carrier and guard tone sit inside the primary band; the level never fell. The source keeps that silence; G must too.

## Measured (tests in primary.rs, 1.5 s in release)

- Every symbol rate at its fastest, a middling and its slowest data rate (2400 to 33 600), phase 3 then three pages with 1 to 2.2 s of control channel and silence between, 30 ms delay, 22 to 60 ppm, noise from 27 dB at 2400 bit/s to 45 dB at 33 600: TRN well, B1 without an error, every bit right, pages trained to 33 dB (4800 on a 28.7 dB line) up to 47.5 dB (33 600 on 45.2 dB).
- 70 000 octets in one page at 33 600 (16.7 s, 45 ppm) and 28 800 on 3000 baud (55 ppm): whole, no slips.
- A 20 ms slip in a page: found by the receiver, the frames found again by the acquirer, everything before it and from 1 s after it right; lost 5 ms (3429 insert), 11 ms (2800 insert), 44 to 52 ms (3429, 2743, 3200 drops).
- **Blind spot, documented in a test:** 20 ms is 48 or 60 whole symbols and 32, 36 or 40 whole carrier cycles at 2400 and 3000 baud. A drop there, or an insert with only concealment's fades, leaves no jump for the receiver's loops, and the decoder's cost barely moves: at 2400 the mapping frames still line up and only Table 12's inversions, half a data frame out, garble a frame every few; at 3000 the frames are half a mapping frame out and the rest of the burst is wrong. T.30's PPR recovers it. **Wanted in data.rs:** a `Decoder` reading of how often its forced Y0 inversion disagreed with a free one -- the acquirer's own test -- so the recipient can search without waiting for a cost that never comes.
- TRN at sixteen points for 127 steps (4.445 s, 80 ppm): all ones, phase 3 over at the count; TRN of no length trains at either size. No S: `Phase3Over { well: false }` at 2.00 s. Random symbols for TRN, or four-point TRN where sixteen were asked: not well.

## For package D

Ask INFOh for TRN of 7 steps or more if phase 3's slip retry is to have TRN to search (wp-c.md); either size trains alike; power reduction 0 as today.

## For package G

- Fill `Channel` from INFOh and `DataMode` from MPh with `nonlinear: false`, `precoding: [(0, 0); 3]`. Own mode: `Mode::Call` if this modem placed the call.
- Source: `phase3()` on INFOh; `page()` when 105 drops out of the control channel's 4T of ones; push page bits, `end_page()` after RCP; `sending() == Idle` is the moment to begin 12.6.1.1's 70 ms. Sum `next_sample()` with the control channel's always.
- Recipient: `expect_phase3()` after INFOh, act on `Phase3Over { well }` (12.3.3); `expect_page()` once this end's 4T of ones have gone, `PageStarted` is 109 on, `take_bits()` are the page's from B1's end, `PageEnded` is 109 off and the turn to 12.6.2 -- but keep the control receiver hunting for Sh throughout, for a burst that never trains. `stop()` on this end's own 105.
