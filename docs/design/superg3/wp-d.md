# WP-D: half-duplex phase 2 (12.2), both roles

Branch `wp/d`, on `wp/ab`. `crates/datapump/src/v34/phase2h.rs`, a sibling of `phase2.rs`, which
changes only in visibility (its constants, `Presence`, `Speaking`, `Modem::capabilities` and the test
`Line` are `pub(crate)`): duplex is unchanged by a sample and its tests pass as they were.

**API.** `phase2h::{Modem, Part::{Source, Recipient}}`, re-exporting `phase2::{Role, Status}`.
`Modem::new(role, part, fs)` starts where phase 1's 75 ms of silence end (INFO0 goes at once);
`step(line) -> f64`; `status()` Running / Done / Failed -- the one failure is the 20 s cap, since
12.2 has waits with no end (12.2.1.3.2); `phase()` for the window. When Done: `infoh()` (sent or
received: symbol rate, carrier, pre-emphasis, power reduction, TRN length in 35 ms steps with
`trn_symbols()`, TRN size), `far_capabilities()` (the far INFO0), `capabilities()` (ours),
`reading()` (the recipient's `probe::Reading`; None at a source), `blind()` (INFOh went with no
source tone heard), `recoveries()`, `info0_repeats()`. Retrain (12.7): `Modem::retrain(role, part,
fs, ours, far)` or `m.again()` -- 70 ms of silence, the tone, then the exchange from 12.2.x.x.3 with
no INFO0. 12.3.3: `m.infoh_again()` on a recipient whose phase 3 failed -- its tone at once, the
source's awaited, then the same INFOh again.

**Chains.** One source chain and one recipient chain, each on either role's tones. Source: INFO0,
tone; far tone then its reversal; +40 ms own reversal, 10 ms of tone, L1 160 ms, L2; far tone over
the L2 echo (`Presence`); tone; INFOh -> Done, silent (12.3.1's 70 ms is G's). Recipient: INFO0,
tone; far tone, and own tone >= 50 ms counted from the INFO0's drain plus the pulse's 10 ms;
reversal, 10 ms, silence; far reversal; L1 passes, 400 ms of L2 read (the tone comes 570 ms after
the reversal, inside the figures' 670); tone; far tone; 25 ms; INFOh; silence -> Done. Bit 28 goes
to 1 once the far INFO0 is read. Recoveries: far tone 30 ms steady with no far INFO0, or a repeated
far INFO0 with bit 28 = 0 -> INFO0 again; source 2700 ms without the far tone -> tone, re-probe;
source 2000 ms without INFOh -> tone kept, far tone awaited gone-and-back; recipient 2000 ms without
the source's reversal -> far tone, own tone, reverse again; recipient 2000 ms without the source's
tone -> INFOh blind.

**INFOh policy** (`settle`). Symbol rate, carrier and pre-emphasis by duplex `settle`'s rule for its
free direction: `Reading::probed` per rate on an INFO0 that is the source's ANDed with ours (bits
12-18; 19 is the source's), `wide` = both have 1664 points; the highest `max_rate` wins, the lower
symbol rate on a tie. Power reduction 0 (and 0 by rule when the source's bit 20 is 0). TRN 29 steps
= 1015 ms: duplex's second, four times the 512 symbols C's slip retry searches. TRN on 16 points when
the projection at the chosen rate exceeds four bits a symbol (`max_rate * 2400 > 4 * S`): four bits
through the 6 dB gap is 18 dB of SNR, what deciding 16 points takes, and at or under it the page goes
at 14 400 or less. No reading or nothing the source can send -> 2400 Bd, low carrier, four points.

**Departures.** (1) A `Steady` detector (60 Hz, like the reversal detector's) judges "far tone" in
the INFO0 and tone stages: `Presence`'s 10 Hz read the far INFO0's DPSK as a tone held 20 ms, which
fired R1 and then took a repeated INFO0's body for the tone's reversal. A tone is 30 ms without a
dip; a reversal counts only if the carrier was steady until it (`ended` within 8 ms). `Presence`
still judges the tone over the L2 echo and the gone-and-back of R4. (2) A repeated far INFO0 with
bit 28 = 0 is answered in the recipient's silent wait for the source's reversal too, and the
recipient goes back to its tone: 12.2.1.4.1 names steps 2-3 only, but a source short of INFO0a
listens for no reversal, and R2's 2000 ms re-reverses 50 ms after the tone, before the next INFO0c
can arrive -- measured as a deadlock until the 20 s cap. (3) A source takes INFOh in any stage once it
has probed (it is set aside before), so a blind INFOh arriving after R4, or after a 2700 ms re-probe,
ends phase 2 rather than being waited past. (4) L2 read 400 ms, not duplex's 300: the source's wait
here is 2700 ms, and the one reading gets 19 windows. (5) INFOh's `high_carrier` at 3429 Bd is
whatever `probed` returned (the carriers coincide, F-V34 #18).

**Tests** (`phase2h::tests`, 12, ~1.2 s in release, on `phase2::tests::Line` plus shaping and
faults): both chains over a short line, the 750 ms VoIP line with echo, and the same with 20 ms
jitter-buffer inserts every 700 ms; INFOh against flat / tilted / narrow / noisy lines and a lesser
source's INFO0; a retrain from either end; a lost INFO0 from either end; the source's reversal in a
hole (R2 + R3); the recipient's reversal in a hole (R2); INFOh in a hole (R4 + `infoh_again`, the
harness standing in for 12.3.3); the blind INFOh; the figures' timings (turn 30-50 ms, own tone
>= 50 ms, L1 at +10 ms, tone within 670 ms, INFOh 25 ms after the tone).

**For G.** Drive `step` until Done; then `infoh()`, `far_capabilities()`, `capabilities()` feed
phase 3 (E) and MPh (F/B): `probe::carriers(rate)` gives the carrier in hertz. The source is silent
on Done and 12.3.1's 70 ms start there; the recipient is silent from its INFOh's end. On 12.3.3 at
the recipient call `infoh_again()` and step it; the source needs nothing, it is still in phase 2.
For 12.7 build `retrain`/`again()` at the moment G wants the 70 ms of silence to begin (responding:
after 50 ms of the far tone, which G's own detector must hear). `phase2h` never detects the far
end's retrain tone in data or the control channel; that is G's (`RetrainWatch`-like). Wanted
elsewhere: `probe::transmits` could be `pub(crate)` (the AND of INFO0s stands in for it here).
