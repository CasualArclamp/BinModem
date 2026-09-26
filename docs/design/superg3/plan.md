# Super G3: V.34 half-duplex fax -- the plan

Started 2026-09-26, when Rory asked for "the 33.6k fax mode like the Kyocera
ECOSYS MA2600cwfx" and sent a screen recording of a V.34 fax calling BinModem
and getting nowhere. Branch `superg3`, worktree
`F:\dialupmodem2\.claude\worktrees\superg3`. Nothing of the V.34 half-duplex
modem reaches main until a whole call works, because `run.ps1` builds the main
working tree and live calls run from it.

The sibling notes:

- `spec-v34-hdx.md` -- what V.34 clauses 10.2 and 12, T.30 clause 6 and Annex F,
  and V.8 say, with every table read off the rendered PDF page.
- `code-v34.md` -- the existing duplex V.34 code, file by file, and what of it
  half-duplex can use.

## 1. What a Super G3 call is

Fax over V.34's half-duplex mode (T.30 Annex F on V.34 clause 12). Once V.8 has
agreed V.34 half-duplex, a call has two channels, used in turn:

- **the control channel**: full duplex, 600 baud QAM at 1200 bit/s (optionally
  2400), the calling modem on a 1200 Hz carrier and the answering modem on
  2400 Hz with an 1800 Hz guard tone. Every T.30 frame goes over it as HDLC --
  NSF, CSI, DIS, TSI, DCS, CFR, PPS, MCF, PPR, DCN. Both ends keep it up and
  send flags between frames; the recipient is silent only while the page
  arrives (F.3.1.2).
- **the primary channel**: one way, source to recipient, at up to 33 600 bit/s,
  V.34's data mode on the symbol rate, carrier and pre-emphasis the start-up
  chose. It carries the page as T.4 Annex A's ECM frames (ECM is mandatory,
  6.1/T.30), then RCP. Each burst begins with its own short resynchronisation
  (S, S-bar, PP, B1) and ends with 35 ms of scrambled ones.

There is no TCF and no FTT (F.3.2.1): the DCS is answered with CFR at once, and
the primary channel's rate is whatever the MPh exchange on the control channel
settled. A rate change is a control-channel restart with new MPh sequences.

The start-up, from Figure F.5-1 (calling end sends the page):

```
caller:   CM CJ | INFO0c  B B L1 L2 B | S S PP TRN | PPh ALT MPh MPh E | flags TSI DCS flags 1s | S S PP B1 page...
answerer: ANSam JM | INFO0a A A  ... A INFOh |      | PPh ALT MPh MPh E | NSF CSI DIS flags CFR flags |
```

## 2. What happened on 2026-09-26, and what was done about it at once

The recording (`tests/vectors/fax-v34-cm.wav`, four seconds of it) shows the
caller repeating one V.8 call menu, `E0 81 85 D4`: transmit facsimile; V.34
half-duplex, V.17, V.29, V.27 ter. BinModem had answered with T.30's plain
2100 Hz tone, which the caller took for ANSam (7.2/V.8 forbids a CM without
it), and then sent its DIS straight past a caller waiting for a JM.

Commit 260cfbd (branch `fax/v8-answer`, the base of this branch): the answering
fax overhears. A silent V.8 answerer listens on V.21's low channel while the
fax sends its tone and DIS; on two identical call menus for sending a fax it
sends a JM of what the two ends share -- without V.34 -- and after CJ, T.30
restarts from its DIS (6.1.6/T.30). That call now goes through at V.17. This
plan is the rest: offering V.34 in that JM, and carrying the call at 33 600.

## 3. What exists and what does not

From `code-v34.md`, in brief:

- **No echo canceller anywhere in V.34** -- it separates directions by
  frequency in phase 2 and relies on a four-wire-like path after. Half-duplex
  needs none: the primary channel is one way and the control channel is FDM.
- **Reusable as it is:** INFO framing and CRC (`info.rs`), phase 2's DPSK,
  tones, reversal and presence machinery (`dpsk.rs`, `phase2.rs` internals),
  L1/L2 generation and analysis with `Reading::probed` (`probe.rs`), S, S-bar,
  PP, TRN at 4 or 16 points (`signals.rs`), the primary-channel transmitter
  (`qam.rs`) and receiver (`receiver.rs`, which keeps its state across
  `idle()`), the data mode encoder, decoder and acquirer built from `Params`
  (`data.rs`, `frame.rs`, `shell.rs`, `trellis.rs`), MP framing, CRC and the E
  finder (`mp.rs`), clause 7's scrambler (`v32::Scrambler`), and 10.2.4's
  differential mapper (`signals::Sender::differential`,
  `Reader::differential`).
- **Small changes:** INFOh (`info.rs`, `dpsk.rs` lengths), MPh (`mp.rs`), the
  receiver's references (16-point TRN after PP; a PP-only resync that lands on
  B1).
- **New:** the 600 baud control channel modem (a complex version of `dpsk.rs`'s
  modulator; a receiver on `dsp::qam::Core` behind a sharp channel-select
  filter); PPh, AC, ALT and their detectors; half-duplex phase 2's four stage
  chains (call/answer x source/recipient); the primary channel's phase 3 and
  page bursts with B1 and turn-off, and a page-end detector; the procedures of
  12.4 to 12.8; a half-duplex top level; V.8 with ANSam for a V.34 fax at both
  ends; T.30 Annex F in `fax::call`; a V.34 branch in `FaxCall`; the window.

## 4. Where the code goes

```
crates/datapump/src/v34/
    info.rs        + InfoH                                        (WP-A)
    dpsk.rs        + INFOh lengths                                (WP-A)
    mp.rs          + Mph, one-direction rate choice               (WP-B)
    receiver.rs    + PpThenTrn(Size), Reference::Pp               (WP-C)
    control.rs     new: the control channel modem, PPh/AC/ALT     (WP-F)
    phase2.rs      + half-duplex stage chains, or a sibling        (WP-D)
    primary.rs     new: phase 3 and page bursts, both ends         (WP-E)
    halfduplex.rs  new: the half-duplex modem, 12.1 to 12.8        (WP-G)
crates/fax/src/call.rs          + Annex F                         (WP-H2)
crates/modem/src/faxcall.rs     + V.8 for V.34, the V.34 branch   (WP-H1, H3)
crates/gui/src/faxwin.rs        + offer V.34                      (WP-H3)
```

## 5. Work packages

Sizes are rough (from `code-v34.md` section 4), in lines of code plus tests.

| WP | What | Depends on | Size |
|---|---|---|---|
| A | INFOh: codec, both DPSK sides, the exhaustive test matches | -- | ~60 + 60 |
| B | MPh Types 0 and 1: codec on MP's helpers, `Finder` hook, 12.4.1.3's rate rule | -- | ~80 + 60 |
| C | Receiver: `PpThenTrn(Size)` for 16-point TRN; `Reference::Pp` for 12.5, next symbol on B1 | -- | ~40 + 100 |
| F | Control channel modem: transmitter (complex 600 baud, carriers, guard tone, Figure 13 pulse), AC/PPh/ALT/Sh/S-bar-h/E/MPh/data symbol sources, turn-off; receiver (channel select, `qam::Core`, PPh(+ALT) training, AC/PPh/Sh detection, differential decode, descramble, E and MPh via `mp::Finder`) | -- | ~700-1000 + 500 |
| D | Half-duplex phase 2: four stage chains, INFOh fill policy (probe reading -> symbol rate, carrier, pre-emphasis; TRN length and size; power reduction 0), 2000/2700 ms recoveries | A | ~350-500 + 300 |
| E | Primary channel: source (phase 3: 70 ms, S 128T, S-bar 16T, PP, TRN; page: 70 ms, S, S-bar, PP, B1, data, 35 ms of ones) and recipient (hunt, train, resync, B1, data, page end by 6.6.2), MPh -> `Params` | C | ~500-700 + 400 |
| G | Half-duplex top level: phase 2 -> phase 3 -> 12.4 control start-up -> (primary burst, 12.6 resync or restart)*, 12.7 and 12.8 retrains, 3 s timers; an API of control bits in/out, page bits in/out and events | B, D, E, F | ~600-900 + 400 |
| H1 | V.8 for a V.34 fax: answerer sends ANSam (6.1.1, 2.6-4.0 s), CM -> JM with V.34 half-duplex; caller hears ANSam, sends CM, JM -> CJ; clause 5 when V.34 is not shared; DIS bit 6 | G (to hand over to) | ~200 + 200 |
| H2 | T.30 Annex F in `fax::call`: phase B on the control channel, no TCF/FTT, CFR, the 40 ones, primary bursts of sync + ECM frames + RCP, post-message on the control channel, rate change by restart, EOR/ERR, DCN; tested against a bit-level stand-in for the modem | JBIG merged (same file) | ~500-800 + 400 |
| H3 | `FaxCall`'s V.34 branch and the window: a "V.34" box, the primary channel's scope, rate and symbol rate | G, H1, H2 | ~300 + 200 |
| I | Whole calls: our caller against our answerer at every symbol rate, over noise, delay, ppm and VoIP slips; the recorded call menu answered with V.34 | H3 | tests |

## 6. Waves

Packages in one wave never edit the same file.

| Wave | Packages | Notes |
|---|---|---|
| 1 | A+B (one agent), C, F | all independent |
| 2 | D, E, H2 | H2 after JBIG is merged, since both edit `fax::call` |
| 3 | G, H1 | |
| 4 | H3, I | the first whole Super G3 call, ours against ours |

The loop, as for V.92 but lighter: one agent per package in its own worktree,
branching `wp/<id>` from `superg3`, editing only its files, passing
`cargo test --workspace --release` and `cargo clippy --all-targets -- -D
warnings`; then merged into `superg3` and the suite run on the result. Every
agent re-reads the rendered PDF pages for anything positional rather than
trusting the digest (see [itu-spec-text-is-lossy]).

## 7. Testing without a Super G3 machine on the bench

- Loopbacks, ours against ours, over the existing channel models (`training.rs`'s
  `Link`: delay, noise, ppm, VoIP slips; `phase2.rs`'s `Line` with echo).
- The recorded call menu: once V.34 is offered, the answer to it is a JM with
  V.34 half-duplex, and the call goes into phase 2.
- Live, in stages, each stage capturing what the far end sends next. Rory's
  line reaches real V.34 faxes both ways: the one that called on 2026-09-26, and
  any machine he can dial. Worth asking for as each wave lands:
  1. after H1 (answer side): a call from a Super G3 fax, answered with ANSam and
     a JM offering V.34 -- the capture has the caller's INFO0c, tone B, L1/L2
     and, if we get that far, S, S-bar, PP and TRN;
  2. after G: the same call carried to the control channel -- the capture has
     PPh, ALT, MPh and E from a real modem, and its NSF/CSI/DIS or TSI/DCS;
  3. after H3: a whole page.

## 8. Decisions and open questions

The full list is `spec-v34-hdx.md` section F. The ones that shape the code:

1. **PPh (equation 10-2) is built as `exp(j pi (2k(k - I) + 1) / 4)`, i = 2k + I.**
   The 02/98 PDF prints `2k(k-1)+1`, and at 600-800 dpi the glyph is a digit one,
   not a capital I. But the clause defines I ("I = 0, 1 for each k") and the
   printed formula never uses it, and the two readings are very different
   sequences. As printed, PPh is `p p p p n n n n` four times over: a square wave
   on one diagonal, with periodic autocorrelation sidelobes of 0.5 and 1.0 --
   a poor thing to train an equaliser on, which is what 12.4 uses PPh for. Read
   with I, it is a perfect sequence (every periodic sidelobe zero) on the four
   diagonal points, of the same family as duplex PP (10-1, which is built on
   k*I). So the I reading is the default, the printed one is kept beside it
   under its own name, and the control-channel receiver looks for either and
   says which it found. **The first capture of a real modem's control-channel
   start-up settles it.**
2. **T at two rates.** S, S-bar, PP, TRN and B1 are primary-channel symbols; PPh,
   ALT, AC, E, MPh, Sh, S-bar-h and the 4T of ones are 600 baud control-channel
   symbols.
3. **The figures' "Current => Correct" insets are followed**: no 2000 ms cap on
   PP + TRN (Figs 23, 24; TRN up to 127 x 35 ms); the recipient's PPh answers Sh
   followed by S-bar-h (Fig 26); the two E sequences are sent side by side (Fig 27).
4. **Unstated details, settled here:** ALT starts with a zero and the control
   channel's differential encoder starts at Z = 0 (the receiver loses at most a
   symbol either way); control-channel data after E stays differentially encoded;
   the source sends MPh Type 0 with bits 29-32 zero; a resync (12.6) carries
   rate, precoder and data-mode parameters over; 12.8.1's AC with no PPh in
   answer falls to the same 3 s rule as everything else.
5. **V.8:** a half-duplex-only fax offers modn0 b7 alone (item 2); offering V.34
   duplex too would select duplex (lowest item wins). ANSam for a fax lasts
   2.6-4.0 s (T.30 6.1.1), not V.8's 5 s.
6. **T.30 in V.34 mode:** no TCF, FTT, CTC or CTR; DCS bits 11-14 are zero; T2
   restarts at each frame rather than on flags; the synchronisation of A.3.1/T.4
   is 200 ms (+100 ms) of flags after B1; the source's "consecutive 1s" before a
   page run until the recipient goes quiet (or its flags stop) and at least 40
   have gone -- which over this rig's VoIP line is 1.5 s or more of them
   ([voip-line-round-trip]).

## 10. The seams between packages

### 10.1 T.30 Annex F (`fax::call`) and the join (`FaxCall`)

`fax::call` knows no signals, and keeps it that way: it says what the line
should be doing through `Line`, and takes bits and a few facts back. In an
Annex F call (entered once V.8 has agreed V.34 half-duplex) the words mean:

| `Line` | the V.34 modem is |
|---|---|
| `V34Control` (frames going) / `V34Listen` (flags only) | on the control channel, sending the bits `next_control_bit` gives -- flags whenever the procedure has nothing else, since F.3.1.2/F.3.1.4 keep the channel busy -- and hearing the far end's at the same time |
| `V34Ones` | on the control channel, sending binary ones (F.3.2.3, F.3.4.5); the procedure counts them and watches for the far end to fall silent |
| `V34Primary` | the source: leaving the control channel (circuit 105 off: 4T of ones), then 70 ms, S, S-bar, PP, B1, and the page bits `next_fast_bit` gives |
| `V34PrimaryListen` | the recipient: silent (4T of ones, then nothing, 12.6.3.2), waiting for the primary channel, handing up its bits |

and back from the join: `control_bit` (descrambled HDLC bits, as now),
`set_control_carrier`, `fast_bits` and `set_fast_carrier` for the primary
channel, and one new fact, that the far end has gone quiet on the control
channel (for the source's ones). The primary rate is the modem's (from MPh),
reported for display; DCS bits 11-14 go as zero (Note 33). Timers follow
Annex F (T2 restarts at each frame, F.3.2.3 Note 2).

The turnarounds belong to the modem, not to T.30: going from `V34Ones` to
`V34Primary` is circuit 105 dropping, and the modem does 12.6.3.1 and 12.5.1;
going from `V34Primary` back to `V34Control` after RCP is 105 dropping again,
and the modem does 12.5.3.1 and 12.6.1 (or 12.4 when a rate change is wanted --
a `renegotiate` request from the procedure, e.g. after PPRs, since CTC/CTR are
gone).

### 10.2 The half-duplex modem (package G) and the join

`halfduplex::Modem::new(role, source, fs)` with `step(input) -> output`, a
state of `Starting | Control | ToPrimary | Primary | ToControl | Retraining |
Failed`, control bits in and out while in `Control`, page bits in or out while
in `Primary`, `to_primary()` and `to_control(renegotiate: bool)` for the
turnarounds, `far_silent()`, and the rates (primary, control). The join (H3)
maps section 10.1's `Line` onto these.
