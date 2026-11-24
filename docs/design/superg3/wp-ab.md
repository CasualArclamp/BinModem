# WP-A+B: the INFOh and MPh codecs

Branch `wp/ab`. Tables 22-24 re-read on the rendered pages agree with the digest; test CRCs are from a separate Figure 14 model.

**API.** `info::{InfoH, INFOH_BITS, Info::InfoH}`: `power_reduction`, `trn_length` (35 ms steps),
`high_carrier`, `pre_emphasis`, `symbol_rate`, `trn_size: signals::Size`; `to_bits`/`from_bits`
(refuses symbol rates 6 and 7); `trn_symbols()` is exact (35 ms = 84a/c symbols).
`dpsk::Receiver::half_duplex(side, fs)` hears INFO0 and INFOh from either side (`Side::lengths(true)`,
an explicit INFOh arm in `decide`); `Receiver::new` is unchanged. `mp::Mph`: `max_rate`,
`control_rate` (bit 27: the rate for the *far* transmitter), `trellis`, `non_linear`,
`expanded_shaping`, `rates` (35:48), `asymmetric_control` (50), `precoding` (Some = Type 1);
reserved bits go as 0, are not read, but are under the CRC. MP and MPh share `head`/`tail`/`check`
(MP's bits unchanged). `mp::primary_rate(source, recipient) -> Option<u8>`: the highest rate enabled
in both masks at or below both maxima, `None` if none (15 counts as 14). `mp::control_rates(ours,
far) -> (transmit, receive)`: the far end's bit 27 only if both set bit 50, else both the lower.
`mp::ControlRate::bits_per_symbol` is 2 or 4. `mp::MphFinder` yields `MphFound::{Mph, E}`.

**Departures.** (1) The code map says INFO0 and INFOh "cannot be confused by length". Wrong: a CRC
register fed its own contents empties, so a quarter of INFOh begin with a valid INFO0 (measured
5024/20000; e.g. 3200 Bd, 16-point TRN, pre-emphasis 0, low carrier). So a half-duplex receiver
hands an INFO0 over only with its first two fill ones (`dpsk::FILL_HEARD`), 3.3 ms after the CRC;
after a false one come two zeros. Duplex receivers are left alone and do not listen for INFOh:
waiting there broke 8 whole-call `v90_call` tests (bisected: timing shifts against their periodic
slips and holes), and an INFO0d begins with a valid INFOh once in 2048. (2) The mirror of (1): an
INFO0 with two zeros where its fill starts *is* an INFOh, so one DPSK symbol error there swaps an
INFO0 for an INFOh (or a colliding INFOh for an INFO0) where elsewhere it would lose the sequence.
(3) `MphFinder` allows a one before the 17-one sync (duplex `Finder` does not): the first MPh follows
ALT, an ALT from 0 (plan decision 4) ends on 1, and it may be the only MPh (12.4.1.3, 12.4.2.4).
(4) Trellis 3 (reserved) reads as 16 states, as in MP; INFOh pre-emphasis 11-15 is kept as sent.

**For D.** Use `Receiver::half_duplex(far side)`: a recipient hears only INFO0, a source INFO0 and,
after probing, INFOh; set aside whatever the stage is not waiting for (see (2)). INFOh's power
reduction is 0 if the source's INFO0 bit 20 is 0; bits 15-18 bound its carrier, 14 and 19 its 3429.
**For C/E.** Pass `trn_size` to `PpThenTrn(Size)`/`Sender::trn`; count TRN with `trn_symbols()`.
**For F.** Feed `MphFinder` the far end's descrambled bits. **For G.** Source: Type 0, bits 29-32
at defaults; recipient: Type 1. `max_rate` at most 12 unless the far INFO0 has bit 25;
`asymmetric_control` false (T.30 F.3.1.4 NOTE); `primary_rate` `None` is not covered by the spec.
Precoding starts at 0 each start-up; a Type 0 keeps it. E is any 20 ones: take it only where due;
after ALT (12.6) it can come a bit early, its last one arriving as data.
