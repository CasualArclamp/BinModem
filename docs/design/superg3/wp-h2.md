# WP-H2: T.30 Annex F in `fax::call`

Branch `wp/h2`. `fax::call::Call` runs T.30 Annex F beside clause 5 -- the
same struct, frames, ECM and codings, the Annex F parts in `call/annex_f.rs`.
Nothing of it knows a signal; the join maps `Line` onto the modem (plan 10.1).

## The API

- `Call::start_annex_f()`: for the join to call once, at either end, as soon
  as V.8 has agreed V.34 half-duplex. Both ends begin phase B with the control
  channel taken as up -- the answerer queues CSI and DIS, the caller waits for
  them -- and whatever clause 5 had going out is dropped. ECM is on from here
  (F.3), T1 counts from here (F.3.2.3 Note 1). `annex_f()` says whether it was.
- `Line::V34Control` / `V34Listen` (the same thing to the modem: on the control
  channel, sending what `next_control_bit` gives), `V34Ones` (the source's
  ones), `V34Primary` (the source's page burst), `V34PrimaryListen` (the
  recipient silent for it). `next_control_bit` always has a bit in Annex F --
  flags when there is nothing to say, ones in `V34Ones` -- until `None` after
  the DCN, which is the moment to drop the line (F.3.4.5 Note 2).
- Back from the join: `control_bit`, `fast_bits`, `set_fast_carrier` as now;
  `set_far_silent(bool)`, the far end's control carrier gone (level, not edge:
  say `false` again when the channel comes up); `set_primary_rate(bps)` after
  every MPh exchange (it sizes the 200 ms of flags that open a page, A.3.1/T.4;
  33 600 assumed until told); `control_restarted()` every time the control
  channel comes back -- after the start-up, each resync and any retrain --
  *before* taking bits, so a burst the restart cut into goes again whole
  behind F.3.1.4's flags (`frames::V34_FLAGS`, four); `renegotiate()`, read as
  the line leaves `V34Primary`: `true` asks for 12.4's start-up with a new MPh
  rate instead of 12.6's resync. Constants `ONES` (40) and `FLAGS_GONE` (0.1 s
  without a flag is the recipient's flags having stopped).

## Departures, and why

- DIS over the control channel: bit 6 set (true, and harmless; unstated), bit 7
  clear (256-octet frames preferred), bits 11-14 still the clause 5 ladder;
  DCS: bits 11-14 zero (Note 33), bit 28 clear, bit 27 always. Notes 23/24 are
  Annex C's and are not applied. Bit 67 is zero by the field's length.
- T2 is put back at the start of a frame -- address and control field heard
  after a flag -- not at each flag (F.3.2.3 Note 2), so Figure F.5-4's "T2
  elapsed" before the DIS again happens. Frame starts alone, never flags.
- The 4th PPR (F.3.4.5 Note 1, CTC/CTR gone): the frames go again once more,
  `renegotiate()` is raised for the control-channel start that follows them,
  and the PPR count starts afresh at the modem's new rate; a second fourth PPR
  on the same block, or one already at 2400, is EOR. DCN is what an unanswered
  EOR gets, as any command. FTT and CTC/CTR arriving are ignored.
- Nothing pauses 75 ms: the channel is up at both ends and the 70 ms gaps
  are the modem's. A DIS while the DCS is unanswered is the DCS again.

## For G (the modem) and H3 (the join)

- `V34Listen` and `V34Control` are one state; only `V34Ones -> V34Primary` and
  `V34Primary -> V34Control` are turnarounds the modem acts on.
- The recipient that times out while silent (`Receiving`) goes to `Ending`
  with a DCN queued on a channel that is down; the join must bring the control
  channel back (12.8) or hang up. The modem must not take control bits it will
  not send (a lost tail is a lost frame; T4 recovers it, at a cost).
- `crates/fax/tests/annex_f.rs` has a bit-level stand-in modem (control both
  ways, primary one way, V.34's turnaround lengths, round trip to 1.5 s) and
  checks Figures F.5-1 to F.5-4 and F.5-7 as what each end sent.
