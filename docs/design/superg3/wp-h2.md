# WP-H2: T.30 Annex F in `fax::call`

Branch `wp/h2`. `fax::call::Call` runs T.30 Annex F beside clause 5 -- the
same struct, frames, ECM and codings, the Annex F parts in `call/annex_f.rs`.
Nothing of it knows a signal; the join maps `Line` onto the modem (plan 10.1).

## The API

- `Call::start_annex_f()`: for the join to call once, at either end, as soon
  as V.8 has agreed V.34 half-duplex. Both ends begin phase B with the control
  channel taken as up (the answerer queues CSI and DIS, the caller waits for
  them); clause 5's queue is dropped, ECM is on (F.3) and T1 counts from here
  (F.3.2.3 Note 1). `annex_f()` says whether it was called.
- `Line::V34Control` / `V34Listen` (one thing to the modem: on the control
  channel, sending what `next_control_bit` gives), `V34Ones` (the source's
  ones), `V34Primary` (the page burst), `V34PrimaryListen` (the recipient
  silent for it). `next_control_bit` always has a bit in Annex F -- flags when
  there is nothing to say, ones in `V34Ones` -- until `None` after the DCN,
  which is the moment to drop the line (F.3.4.5 Note 2).
- Back from the join: `control_bit`, `fast_bits`, `set_fast_carrier` as now;
  `set_far_silent(bool)`, the far end's control carrier gone (level or edge;
  forgotten as the line begins to turn round and when the channel comes back);
  `set_primary_rate(bps)` after every MPh exchange (sizes A.3.1/T.4's 200 ms
  of flags; 33 600 until told); `control_restarted()` whenever the control
  channel comes back, *before* taking bits, so a burst the restart cut into
  goes again whole behind F.3.1.4's flags (`frames::V34_FLAGS`, four);
  `renegotiate()`, read as the line leaves `V34Primary`: `true` asks for
  12.4's start-up with a new MPh rate instead of 12.6's resync. Constants
  `ONES` (40) and `FLAGS_GONE` (0.1 s without a flag: the flags have stopped).

## Departures, and why

- DIS over the control channel: bit 6 set (true, and harmless; unstated), bit 7
  clear (256-octet frames preferred), bits 11-14 still the clause 5 ladder;
  DCS: bits 11-14 zero (Note 33), bit 28 clear, bit 27 always. Notes 23/24 are
  Annex C's and are not applied. Bit 67 is zero by the field's length.
- T2 is put back at the start of a frame -- address and control field heard
  after a flag -- not at each flag (F.3.2.3 Note 2), so Figure F.5-4's "T2
  elapsed" before the DIS again happens. T4 is put back by nothing, but no
  timer runs out while a frame's start is on the line (`FRAME_SECONDS`,
  0.35 s): the channel carries both ends, so clause 5's hold on the far end's
  carrier has nothing to hold on, and this is what is left of it.
- The 4th PPR (F.3.4.5 Note 1: CTC/CTR gone, "EOR/ERR or DCN signals are used
  to transit", a rate change "at every start of the control channel"): the
  frames go again once more with `renegotiate()` raised for the control-channel
  start after them, and the PPR count starts afresh at the modem's new rate; a
  second fourth PPR on the block, or one already at 2400, is EOR. DCN is what
  an unanswered EOR gets, as any command. FTT and CTC/CTR are ignored.
- Nothing pauses 75 ms: the channel is up at both ends and the 70 ms gaps
  are the modem's. A DIS while the DCS is unanswered is the DCS again.

## For G (the modem) and H3 (the join)

Only `V34Ones -> V34Primary` and `V34Primary -> V34Control` are turnarounds the
modem acts on. A recipient that times out while silent (`Receiving`) queues a
DCN on a channel that is down: the join brings it back (12.8) or hangs up. The
modem must not take control bits it will not send. `tests/annex_f.rs` has a
bit-level stand-in modem (control both ways, primary one way, V.34's turnaround
lengths, round trip to 1.5 s) and checks Figures F.5-1 to F.5-4 and F.5-7.
