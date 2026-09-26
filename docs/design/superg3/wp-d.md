# WP-D: half-duplex phase 2 (12.2), both roles

Branch `wp/d`, on `wp/ab`. `crates/datapump/src/v34/phase2h.rs`, a sibling of `phase2.rs`, which changes
only in visibility (constants, `Presence`, `Speaking`, `capabilities`, the test `Line`): duplex is unchanged.

**API.** `phase2h::{Modem, Part::{Source, Recipient}, Steady}`, re-exporting `phase2::{Role, Status}`.
`Modem::new(role, part, fs)` starts where phase 1's 75 ms of silence end (INFO0 goes at once);
`step(line) -> f64`; `status()` Running / Done / Failed -- the one failure is the 20 s cap, since 12.2 has
waits with no end (12.2.1.3.2); `phase()` for the window. When Done: `infoh()` (sent or received: symbol
rate, carrier, pre-emphasis, power reduction, TRN length in 35 ms steps with `trn_symbols()`, TRN size),
`far_capabilities()`, `capabilities()`, `reading()` (the recipient's `probe::Reading`; None at a source),
`blind()` (INFOh went with no source tone heard), `recoveries()`, `info0_repeats()`. Retrain (12.7):
`Modem::retrain(role, part, fs, ours, far)` or `m.again()` -- 70 ms of silence, the tone, then the exchange
from 12.2.x.x.3 with no INFO0. 12.3.3: `m.infoh_again()` on a recipient whose phase 3 failed -- its tone at
once, the source's awaited, the same INFOh again. `Steady::new(far carrier, fs)`, `feed(x)`, `held()`.

**Chains.** One source chain and one recipient chain, each on either role's tones, in `Stage`'s order:
INFO0 and tone from both; the recipient reverses after 50 ms of own tone (from its INFO0's drain plus the
pulse's 10 ms) with the source's heard; the source answers 40 ms on, 10 ms of tone, L1, L2; the recipient
reads 400 ms of L2 (its tone 570 ms after the reversal, inside the figures' 670) and sends its tone; the
source hears it over the L2 echo and sends its own; 25 ms on, INFOh; the source is Done at INFOh and
silent, the recipient when INFOh has left the line. Bit 28 goes to 1 once the far INFO0 is read. Every
recovery of 12.2.1.3, 12.2.1.4, 12.2.2.3 and 12.2.2.4 is in `stage_step` and `heard`, cited by clause.

**INFOh policy** (`settle`). Symbol rate, carrier and pre-emphasis by duplex `settle`'s rule for its free
direction: `Reading::probed` per rate on an INFO0 that is the source's ANDed with ours (bits 12-18; 19 is
the source's), `wide` = both have 1664 points; the highest `max_rate` wins, the lower symbol rate on a tie.
Power reduction 0, whatever the source's bit 20 (0 by rule when it is 0). TRN 29 steps = 1015 ms: duplex's
second, four times the 512 symbols C's slip retry searches. TRN on 16 points when the projection at the
chosen rate exceeds four bits a symbol (`max_rate * 2400 > 4 * S`): four bits through the 6 dB gap is 18 dB
of SNR, about what deciding 16 points takes, and at or under it the page goes at 14 400 or less. No
reading, or nothing the source can send at -> 2400 Bd, low carrier, four points.

**Departures.** (1) A `Steady` detector (60 Hz, the reversal detector's width) judges "far tone" wherever a
sequence might be on the far carrier: `Presence`'s 10 Hz read the far INFO0's DPSK as a tone held 20 ms,
which fired R1 and then took a repeated INFO0's body for a reversal. A tone is 30 ms without a dip, and a
reversal counts only if the carrier was steady until it. `Presence` still hears the tone over the L2 echo,
and R4's gone-and-back. (2) A repeated far INFO0 with bit 28 = 0 is answered in the recipient's silent wait
for the source's reversal too, and the recipient goes back to its tone: 12.2.1.4.1 names steps 2-3 only,
but left to R2 the two went round until the 20 s cap. (3) A source takes INFOh in any stage once it has
probed, so a blind INFOh after R4 or a re-probe ends phase 2. (4) L2 read 400 ms, not duplex's 300: the
source waits 2700 ms. (5) `high_carrier` at 3429 Bd is whatever `probed` returned (F-V34 #18). (6) INFO0
is repeated once per repeated far INFO0 with bit 28 = 0, not continuously; a VoIP round trip holds twenty.

**Tests** (`phase2h::tests`, 14, ~1.2 s in release, on `phase2::tests::Line` plus shaping and faults): both
chains over a short line, the 750 ms VoIP line with echo, the same at 30 and 6 dB of SNR and with 20 ms
jitter-buffer inserts every 700 ms; INFOh against flat / tilted / narrow / noisy lines, a lesser source and
one that could reduce power; a retrain from both ends at once, and one begun at either end while the other
still talks (600 baud random bits), answered after 50 ms heard and 70 ms silent; a lost INFO0 from either
end on both lines; each wait run out once (R2 + R3, R2, R4 + `infoh_again`, blind INFOh); the figures' timings.

**For G.** Drive `step` until Done; then `infoh()`, `far_capabilities()`, `capabilities()` feed phase 3
(E's `Channel`: `Band::new(symbol_rate, high_carrier)`, `pre_emphasis`, `reduction`, `trn_size`,
`trn_steps = trn_length`) and MPh (F/B). The source is silent on Done and 12.3.1's 70 ms start there; the
recipient is silent from its INFOh's end. On 12.3.3 at the recipient call `infoh_again()` and step it; the
source needs nothing, it is still in phase 2. For 12.7 build `retrain`/`again()` when the 70 ms of silence
are to begin: at once when initiating; when responding, once a `Steady` on the far carrier has `held()`
50 ms (12.7.1.2), which the control channel's 600 baud symbols never let it -- they dip the carrier as a
sequence's reversals do. Wanted elsewhere: `probe::transmits` could be `pub(crate)` (the AND stands in).
