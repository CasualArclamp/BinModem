# WP-H3: the join, the window, and whole Super G3 calls

Branch `wp/h3` (`superg3`, and G's final `wp/g`): the join, `faxcall.rs`; the
window's V.34 box; the whole-call tests, `crates/modem/tests/superg3_fax.rs`.

**Built.** At the hand-over (`v34_agreed()`, 75 ms after CJ) `FaxCall` builds
`halfduplex::Modem::new(role, source, fs)` -- the caller the call modem and the
source; no polling -- and steps it every sample, phase 2 included. Control bits
go 16 ahead while `State::Control`, every bit up to `control_bit`; `ControlUp`
is `control_restarted()` and `set_primary_rate`; `far_silent()` goes to T.30;
page bits 512 ahead; `to_control(renegotiate, or a cap under the rate)` at the
page's end; the recipient turns at once and hands up its page and 109, and its
DCN on a channel that is down is `retrain_control()`; the modem's `Failed`
fails the call with its reason. The source turns when its modem hears the
recipient silent, on T.30's flags-gone alone 0.2 s later; a turn asked between
channels waits, and only `Event::Retraining` undoes one made. The window
(feb5e31): a "V.34" box, on by default; "at 33 600 bit/s on 3429 baud"; the
far-end panel's rows for both channels; the scope blank in phase 2, 4PSK or
16QAM on the control channel, "1408TCM" and so on at the recipient.

**Works end to end** (ours against ours, 16 kHz, 15 dB loss; dial to both ends
done; all eleven tests pass):
- One 300-line JBIG page, 30 ms each way: 33 600 on 3429 Bd, control 1200 both
  ways, trained 53 dB, 10.3 s. Three fine MMR pages, heavy dither: 34.3 s.
- 28 dB of noise: 28 800, trained 32 dB, 10.5 s. The rig's VoIP line (750 ms
  each way, 40 dB, own echo 10 dB down): 33 600, 25.8 s, no retrain.
- 20 ms slips every 2 s and one mid-page in each of two pages: both whole, a PPR
  each, 20.4 s. A rate change asked at the source (24 000) or the recipient
  (21 600): the second page at it, by a start-up, 12.5 / 12.7 s.
- No page: TSI, DCS, CFR, DCN, 7.3 s. Far end gone after the page: failed 13.2 s
  on, "the far end did not answer AC"; source gone before it: 15.0 s on.
- A V.34 end and a plain one either way round: clause 5 at 14 400. The recorded
  call menu: a JM with V.34 half-duplex, INFO0a 0.19 s after CJ.
- (lib) The recipient's 12.7 retrain as the source's ones start, a little noise:
  the page waits it out and goes once (the old join's went into phase 2 too).

**Not working, or not shown.** Nothing failed below the join: both failures
were the tests' (an ear matching only `Heard::Jm`, where `v8::Decoder` names
every menu CM; a page-less call expected to send no DCS). Not shown: a clock
offset in a whole call (the rig measured ~114 ppm via MicroSIP), or a symbol
rate but 3429, which two of ours always choose -- WP-I's; G's tests have 40-50
ppm from phase 3, and its ignored corner stands (wp-g.md 5). Only 16 kHz; no
polling; no cap in the window (`limit_v34_rate` is the tests').

**Departures.** (1) The turn on `far_silent()`: T.30's flags-gone comes 130-165
ms after a far end stops, its 12.7 tone at 70 ms, the modem off to the retrain
at about 155 ms -- a race. (2) A page-less call commands anyway, as `fax::call`
always has. (3) The source's scope is blank during its page. (4) V.34 is on by
default; four clause 5 tests untick it at one end. Outside the join, all in
feb5e31: README.md, docs/usage.md, `crates/gui/src/{app,faxwin,live}.rs`,
`crates/modem/src/lib.rs`, `crates/modem/tests/{call,v17_fax}.rs` and
`crates/telemetry`'s `fax_symbol_rate`. No lower package was changed.

**Live tests for Rory** (plan 7, stage 3), each kept in `dist\captures`: (1) a
Super G3 machine (the MA2600cwfx that called on 2026-09-26, or any) faxing
BinModem one page, V.34 ticked; (2) BinModem faxing it one page; (3) either
with V.34 unticked, which should go at V.17 as before. Each settles PPh's
reading (plan 8.1: the control receiver says which it heard), whether its MPh
and E are read, whether it falls silent for our forty ones, and the page.
