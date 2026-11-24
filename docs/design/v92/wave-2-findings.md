# Wave 2 review findings, not yet answered
Written by the wave-2 reviewers (2026-09-18). Each package was built, passed the whole suite and
clippy, and was then reviewed against re-rendered Recommendation pages. The corrections round that
normally follows was not run, so everything below is still open. Answer it the way wave 1 did: give each
package its own list, re-render the page it cites, fix what is right and reject what is wrong.

## V92-08 (wp/V92-08)
- Reviewer verdict: ok; 3 nit, 2 should-fix
- Plan amended: Yes, in its own commit (b983c5d), as the rules require. Two changes, both in F:\dialupmodem2\docs\design\v92\plan.md.

Section 5, V92-08's entry: the size letter goes from S to M (604 lines across the three files, against S's "up to about 300 lines including tests"); the description gains two bullets for things it had left implicit — how the ADP is spliced between frames and what `Answerer::sendin

**[should-fix] crates/ec/src/stack.rs:2602**

after_resume_the_link_carries_on_with_the_same_sequence_numbers_and_dictionary: the two N(S)
assertions are satisfied by frames that crossed BEFORE the hold, so they cannot detect the re-
establishment their messages name. outbound_ns drains the log, and the log is never drained
between the pre-hold settle and the post-resume one, so `sent` is both batches concatenated.
Instrumented run in a scratch copy: last = 4, pre-hold N(S) = [5,6,7,8,9], post-resume N(S) =
[10,11,12,13,14], sent = [5..14]. `assert_ne!(sent.first(), Some(0))` and
`assert!(sent.contains(&(last + 1)))` (contains 5) are both decided by frame 5. A stack that re-
established during the resume and restarted V(S) at 0 would give sent = [5,6,7,8,9,0,1,...] and
pass both. Proof: draining the log immediately before a.suspend() makes contains(5) fail. The
substantive claim is still carried by the b.take_received() == expected check; only the explicit
V(S) checks are vacuous. Fix: take `last` from a drain done just before a.suspend(), then assert
the first post-resume N(S) is last + 1.

**[should-fix] crates/ec/src/stack.rs:889**

Stack's own half of the suspension - 7.10.1's T400 freeze and the XID exchange's clock - has no
test. Deleting the `if self.suspended { self.lapm.tick(dt_ms); self.drain(); return; }` block
from Stack::tick in a scratch copy leaves all 260 ec tests green (224 lib + 33 loopback + 3
v42bis_real), and nothing outside ec calls suspend/resume yet. Both suspend tests suspend a link
already in Phase::Protocol, where the `match self.phase` arm is empty and the only live clock is
lapm's T401, so they prove Lapm::suspend and nothing else. The plan entry and the section-14
note both single this out ("T400 and the XID exchange's in stack.rs, T401 in Lapm::suspend"). A
test that suspends a Stack in Phase::Detecting, ticks past T400 and asserts phase() is still
Detecting would close it.

**[nit] crates/ec/src/stack.rs:526**

bypassing_detection() does not check self.role. Stack::new(Role::Originator,
..).bypassing_detection() compiles and installs an Answerer watch whose T400 expiry calls
bypass_failed() -> Phase::Transparent at 750 ms, silently undoing without_detection()'s larger
N400 while SABMEs are still going out. V92-52 picks the builder by role, so it is a misuse
hazard for the next package; a debug_assert_eq!(self.role, Role::Answerer) would pin the intent
the doc already states.

**[nit] crates/ec/src/lapm.rs:305**

Lapm::suspended() is public and has no caller anywhere in the workspace (grep for "suspended()"
finds only the two Stack::suspended calls in the stack test). Dead public API duplicating state
only Stack can set.

**[nit] docs/design/v92/plan.md:2883**

The section-14 amendment note says "604 lines across the three files"; the three code commits
add 619 (detect.rs +65, lapm.rs +47, stack.rs +507). The M band (300-800) is right either way.

- Implementer left open: Nothing calls the new API yet, by design. `Stack::bypassing_detection()` is wired up by V92-52
(the quick-connect path) and V92-31, and `suspend()`/`resume()` by the hold packages (V92-55/56)
and, if wanted, around retrains; the modem crate's stack construction at
crates/modem/src/lib.rs:1647 is outside this package's Files line and was not touched. The
trigger for the bypass — both P bits, or both prot0 LAPM plus both INFO0 V.92 bits — therefore
has no home yet either.

- Implementer left open: T402 and T403 are still not implemented, so 7.10.2's "if implemented" clause has nothing to
freeze. That is the pre-existing state of crates/ec (the lapm.rs module doc already says so) and
the plan's entry explicitly allows it.

- Implementer left open: `cargo fmt --check` is not clean on crates/ec, but it was not clean on v92 either — the repo's
hand-formatting differs from rustfmt's defaults in many pre-existing places (bits.rs, detect.rs,
frame.rs), and there is no rustfmt.toml. New code follows the surrounding hand style; clippy
with -D warnings is the gate that was met.

- Implementer left open: The bypass's interaction with a real far end is untested against a capture. V.8 7.3's warning
and the ECEC-arriving-late behaviour in the project memory are the evidence the ODP answer is
built on, but no live V.92 call exists yet to confirm that a bypassing answerer and a detection-
running originator meet correctly on the line.

## V92-09 (wp/V92-09)
- Reviewer verdict: ok; 6 nit, 2 should-fix
- Plan amended: Yes, in its own commit (38f1ade). Two changes to docs/design/v92/plan.md: (1) V92-09's heading is now (M) not (S) -- the file came to 750 lines, a third of them clause quotations -- and its entry now names FrameBits, the three constants, the extra tests, and why R = M-1 is only a legal frame when M = 2^K exactly; (2) a section 14 paragraph, 'V92-09: the alternative reading of step 4 is not a readi

**[should-fix] crates/datapump/src/v92/modulus.rs:695**

`the_first_bit_in_time_is_the_lowest_in_value` cannot fail if the bit order is reversed. Every
value it uses is a palindrome: `from_bits(&[true, false, false, true])` is 9 whether b0 is the
LSB or the MSB, `bits()` reconstructs the same palindrome through `bit()`, `FrameBits::new(9,
4)` is 9 either way, and `labels[0] == 9` follows from the value. Proved by mutation in a
throwaway copy under my scratch folder: changing `value |= 1u128 << i` to `1u128 << (bits.len()
- 1 - i)` (and separately reversing `bit()`) leaves all ten tests green. This is the one test
that pins step 1's "b0 is first in time" reading, and `from_bits` is the API V92-17's
transmitter will hand scrambler output to; a reversed frame is exactly the "plausible and wrong"
failure the module doc warns about. A non-palindromic vector such as [true, true, false, false]
(3 one way, 12 the other) fixes it.

**[should-fix] crates/datapump/src/v92/modulus.rs:233**

The `Encoder` doc justifies keeping the moduli out of the struct with "a rate renegotiation
changes them between one frame and the next without disturbing the sign chain". That is not what
the Recommendation does. Figure 18 (rate renegotiation) runs DATA Ru ... E2u B1u DATA and Figure
19 (fast parameter exchange) runs DATA RM ... E2u FB1u B1u DATA; 8.7.1 then says "The scrambler,
modulus encoder, convolutional encoder, precoder and prefilter memories are initialized to zero
prior to transmitting B1u", and 9.9.2.1.2/9.9.2.2.3 zero them again. There is no moment at which
the moduli change while the sign chain keeps running. The design choice is still fine, but the
reason given is wrong, and V92-17/V92-19 read this comment when deciding whether a parameter
change needs `reset()`.

**[nit] crates/datapump/src/v92/modulus.rs:6**

The module doc opens "Each of the twelve symbols of an upstream data frame carries one of M_i
levels". Pitfall P-3 warns against exactly this: "the number of positive levels" is V.90's M_i
(5.4.1); V.92's M_i is an explicit CP_d byte (Table 30, bits 52:152, "Modulus encoder parameter
M_i") and the interval's constellation has N = 2*LC levels with N >= M_i (and N >= 2*M_i at k =
3). The symbol carries one of N levels; M_i is the number of equivalence classes. The rest of
the module is careful about this, so it is only the opening sentence.

**[nit] crates/datapump/src/v92/modulus.rs:315**

The `Decoder` doc says "The mixed radix goes back together most significant interval last", but
the loop seeds with interval 11 (the largest place value) and finishes with interval 0. The
inline comment eight lines later gets it right ("the mixed radix read from interval 11 down")
and V.90's twin comment in v90/modulus.rs::decode, over identical code, says "most significant
interval first".

**[nit] crates/datapump/src/v92/modulus.rs:385**

Two loose ends around the frame-width guard in `Decoder::decode`. (a) The `"the frame holds a
larger value than the rate carries"` branch has no test: deleting the whole `if r >> k != 0`
block leaves all ten tests green (verified by mutation). (b) `Moduli12::fits` accepts any k up
to 127, but `FrameBits::new` (line 109) `debug_assert!`s len <= LONGEST_FRAME = 72, so
`decode(labels, &Moduli12::new([255; 12]), 90)` passes the `fits` guard and then panics in a
debug build instead of returning a lowercase reason. `Parameters::fits` keeps drn in 1..=19, so
no real path reaches it, but decode's own validation and FrameBits' contract disagree.

**[nit] crates/datapump/src/v92/modulus.rs:722**

The comment in `seventy_two_bits_need_u128` reads "twelve sixes are not enough and twelve sixty-
fours are exactly enough", but the assertion below it tests `Moduli12::new([63; UP_INTERVALS])`
— twelve sixty-threes (63^12 = 3.909e21 < 2^72 = 4.722e21). Twelve sixes would be nowhere near.

**[nit] crates/datapump/src/v92/modulus.rs:187**

`Moduli12::modulus` silently wraps with `self.moduli[i % UP_INTERVALS]`. The doc ("M_i, the
modulus of data frame interval `i`") does not say so, and no test notices: replacing it with
`self.moduli[i]` leaves all ten tests green. `Parameters::modulus` in v92/mod.rs wraps the same
way, so it is at least consistent, but an out-of-range interval index quietly returns M0 instead
of failing.

**[nit] crates/datapump/src/v92/modulus.rs:69**

`D_BEFORE_THE_FIRST_FRAME` states as fact that "9.9.2 zeroes the same memory again at a fast
parameter exchange". 9.9.2.1.2 says "initialize the scrambler and differential encoder to zero"
at a point where the next thing transmitted is SUVu, whose differential encoder is the sign-bit
one of 8.7.5; reading it as the modulus encoder's step-3 memory is an interpretation, which is
how INTRO Q-2 marks it. It is harmless (B1u follows either way, and 8.7.1 covers that), but the
constant presents an interpretation as a second citation.

- Implementer left open: The INTRO digest (docs/design/v92/spec-intro-transmitter.md, Q-1) still says 'Both readings
decode' of step 4, which the package disproves. That file is outside V92-09's Files line, so it
is untouched; the correction is recorded in plan.md section 14 instead. Someone with the digest
in their file list should fix Q-1 and the matching sentence in plan section 4's table.

- Implementer left open: No capture confirms the step-4 reading; 6.4.1 is proved only against itself. The
Recommendation's own text and the non-invertibility of the alternative are the whole of the
evidence until a real V.92 server's B1u is captured (plan section 13).

- Implementer left open: Parameters::product() in v92/mod.rs and Moduli12::product() are the same arithmetic on the same
twelve bytes. mod.rs is outside this package's Files line, so the overlap stands; Moduli12 is
documented as the form the encoder and decoder carry, with the product worked out once.

## V92-10 (wp/V92-10)
- Reviewer verdict: ok; 5 nit, 2 should-fix
- Plan amended: Yes, in a separate commit (533c850), both halves section 2 requires. The entry: V92-10's size letter M became L (v92/precoder.rs is 1128 lines against M's 300-800, the same correction V92-01's entry needed and for the same reason); the description gained the two named readings TIE_TO_SMALLER_INDEX and OUTPUT_LIMIT, the four public map helpers whose one home is here because V92-20 needs them backwa

**[should-fix] crates/datapump/src/v92/precoder.rs:929**

The doc on `an_inverse_channel_gives_back_every_k_as_eta_mod_m` claims it "catches what a
forward-only test cannot: an off-by-one in either feed-forward section", and the amended plan
entry repeats it. It only half-delivers: the test's `z1` is `vec![0.2]` (one tap, line 935), so
only z1(1) is ever exercised, and
`the_prefilter_feed_forward_starts_at_kappa_0_and_the_precoder_s_at_1` also uses a single z1
tap. Proved by mutation in a scratch copy (never in the worktree): replacing
`self.u_back.back(index + 1)` with `self.u_back.back(if index == 0 { 1 } else { index })` in
`Precoder::tail` -- an off-by-one from the second tap onward -- leaves all 13 tests passing. The
same hole exists for `p2` (one tap in every test). `z2` is the only section tested with two
taps. The code itself is right; it is the guard that is thin, and P-5 is exactly the pitfall
this test is named for. Give the inverse-channel test a z1 of two or three taps and a p2 of two.

**[should-fix] crates/datapump/src/v92/precoder.rs:344**

`label_of(first, second)` maps the pair to `(2*first + 1, 2*second + 1)` =
`v34::constellation::Point` = (x, y), i.e. y(0) becomes the real coordinate of Figure 9/V.34.
That is an interpretation: 6.4.3 says only "the two pairs (y(0),y(1)) and (y(2),y(3))", and V.34
9.6.3.1 calls y(2m) a complex symbol whose Figure 9 axes are Re and Im. Figure 9 is not
symmetric (label(-3,1)=100 vs label(1,-3)=100, but label(-1,1)=011 vs label(1,-1)=001), so the
swapped reading gives different Y1..Y4, a different Y0 sequence, and an upstream a conforming
digital modem could not follow. No test can catch it: mutating `inverse_map` to
`label_of(etas[1], etas[0])` / `label_of(etas[3], etas[2])` leaves all 13 tests green, because
`the_four_indices_...` recomputes Y0 through the same `inverse_map`. The reading is almost
certainly right (the pair is written in the order Re, Im), but the doc does not say which of the
pair is the real axis, and V92-20's decoder will be written from this comment. Say it in the
doc, with the V.34 9.6.3.1 "complex 2D channel output symbols" sentence as the reason.

**[nit] crates/datapump/src/v92/precoder.rs:1041**

`the_state_is_never_saturated_only_the_output` cannot detect the thing it is named for. Every
assertion is gain-independent (`soft.x == hard.x`, `soft.v == hard.v`), and the state does not
depend on G at all, so a clamp applied to x(n) or v(n) itself would clamp both chains
identically and the test would still pass. Line 1059's `assert_eq!(hard.out, (1000.0 *
hard.v).clamp(-OUTPUT_LIMIT, OUTPUT_LIMIT))` restates the implementation verbatim and asserts
nothing. Coverage does exist -- clamping the prefilter state or x(n) in a scratch copy fails
`an_inverse_channel_gives_back_every_k_as_eta_mod_m` (and, for x, the long-run test) -- but not
here. Also `clamped` (line 1057) counts symbols with `hard.v != 0.0`, not symbols where the
limit engaged, so the failure message "only {clamped} symbols exercised the clamp" is wrong.
Assert instead that some symbol has |v| > OUTPUT_LIMIT while |out| == OUTPUT_LIMIT.

**[nit] crates/datapump/src/v92/precoder.rs:623**

`// "Y0(m) does not depend on the current frame's inputs"` is in quotation marks but is not
V.34's wording. 9.6.3.2/V.34 (rendered) says: "There is an inherent delay of one 4D symbol
interval in the convolutional encoder. Therefore, the output Y0(m) does not depend on the
current input [Y4(m), Y3(m), Y2(m), Y1(m)]." Every other quotation in the file is verbatim; this
one is a paraphrase wearing quote marks. Same at line 343, where `6.4.3 "calculated as 2 x y(k)
+ 1"` splices a fragment out of "The odd-integer coordinates used in 9.6.3.1/V.34 are calculated
as 2 x y(k) + 1."

**[nit] crates/datapump/src/v92/precoder.rs:566**

`Chain::new`'s doc enumerates three checks ("every interval must have a constellation, the
prefilter must have a feed-forward section ..., and every equivalence class must have a member")
and the amended plan entry repeats the three, but the code makes five: it also rejects an empty
constellation set and a zero modulus. Conversely it does not check that a set is in ascending
magnitude, although `Constellation::nearest` (line 169) uses `partition_point` and silently mis-
selects on an unsorted set -- and that is something the chain itself needs, unlike the gain
bound that `Chain::new` deliberately skips. Either add the ascending check in `fits`'s own
wording, or say in the doc that the caller owes it.

**[nit] crates/datapump/src/v92/precoder.rs:255**

`Class::for_interval` and `index_to_ki` (line 312) both silently apply `modulus.max(1)` to a
zero Mi, and neither doc mentions it. Both are public and V92-20's decoder is named as the
consumer of `index_to_ki`, so a decoder handed a zero modulus gets Ki = 0 back rather than a
report -- the opposite of the package's "reported, never panicked on" rule. Document the clamp,
or take a `NonZeroU8`.

**[nit] crates/datapump/src/v92/precoder.rs:77**

"Eight is 18 dB above the mean square the design works to" compares an amplitude with a mean
square. 20*log10(8) = 18.06 dB is the ratio of the bound to the RMS, not to the mean square. The
number and the intent are right; the sentence is not, and this is the doc a later package will
read to decide whether to raise the limit.

- Implementer left open: OUTPUT_LIMIT = 8.0 is derived, not printed: clause 6 bounds nothing, and P-12 only says to
saturate the final D/A value and never the filter state. It is a bound in the units 8.8.3 fixes
(mean square of G x v(n) equal to 1) and in no others, so a chain run with a trial gain -- which
is how V92-62 finds G -- must measure on Symbol::v and not Symbol::out. The constant's doc says
so, and the plan's named test with_no_filters_the_output_is_the_chosen_level uses a set whose
levels are inside the limit at G = 1 for exactly this reason. It needs a capture to confirm the
headroom, not a further code change.

- Implementer left open: The equivalence-class feasibility check in Chain::new repeats one rule that Parameters::fits in
v92/mod.rs also applies (N >= Mi, and N >= 2 x Mi at k = 3). It is deliberate and documented,
with fits's own error wording reused so there is one vocabulary, but it is two places to edit if
that rule ever changes. Splitting fits so the chain could call the narrower half would have
meant editing v92/mod.rs, which is outside this package's Files line.

- Implementer left open: Figure 9/V.34 and Table 13/V.34 were taken from v34::trellis rather than re-read from the V.34
PDF, as the plan's description directs. If a live V.92 upstream capture ever disagrees on the
subset labelling, that module's tables are where to look, not this one.

- Implementer left open: Nothing outside crates/datapump/src/v92/precoder.rs and docs/design/v92/plan.md was touched,
nothing was pushed, and the worktree is clean.

## V92-11 (wp/V92-11)
- Reviewer verdict: ok; 4 nit, 3 should-fix
- Plan amended: Yes, in its own commit (0a38537), touching docs/design/v92/plan.md only.

Section 5, the V92-11 entry: size M becomes L; the CpFamily bullet now also names DownFamily and says why downstream needs a second enumeration (CPd has no type field at 19:20 - those bits are its part flags - so only the direction chooses the reading); the finder bullet becomes one finder per direction, with the length call

**[should-fix] crates/datapump/src/v92/sequences.rs:730**

`Cpd::check`'s doc says "Zero skips the rate check", which is false: `Parameters::fits()` re-
runs the 2^K <= product-of-moduli check unconditionally with `up_bits(self.drn)`. Proven in a
scratch copy: `Cpd { moduli: Some([2; UP_INTERVALS]), ..Cpd::from(&parameters())
}.check(&limits, 0)` returns Err("the data frame carries more bits than the moduli can hold"),
not Ok(()). The `up_bits` parameter is also redundant - `Parameters::try_from(&Cpd)` copies
`cpd.drn`, so the merged-CPd rationale in the doc cannot arise.

**[should-fix] crates/datapump/src/v92/sequences.rs:1639**

The plan requires `reserved_bits_set_by_a_far_end_are_ignored_not_rejected` to prove every
sequence "parses to the same interpreted fields as the all-zeros case". For Jd it asserts a
CHANGED field instead - `Some(J::Jd(Jd { sixteen_in_renegotiation: true, ..jd }))` - because
`J::from_bits` lets Table 21's reserved bit 48 through into V.90's `sixteen_in_renegotiation`
(V.92 moved that choice to Jp bits 48/49). Fixable inside this package's file by clearing both
V.90 constellation flags on the Jd arm of `J::from_bits`; the bit-exact round-trip assertion
still passes. Silently changed plan requirement, not recorded in the section 14 amendment.
(Separately, the plan's Table 23 runs 26:30, 36:48, 129:135 are not swept here at all; harmless,
V92-03's test of the same name covers them.)

**[should-fix] crates/datapump/src/v92/sequences.rs:574**

Three wire bit transpositions each pass all 21 tests (mutations applied to a throwaway copy in
my scratch folder, never the worktree): (a) swapping CPd's modulus and filter part flags, bit 19
<-> bit 20, in `word0`, `from_bits` and `cpd_words`; (b) moving CPd bit 29 `extend_e2u` to bit
30 (the reserved sweep cannot catch it because the `parameters()` fixture already has
`extend_e2u: true`, so bits 30:32 are unpinned against it); (c) swapping SUVu `silence` (bit 32)
and `ack` (bit 33) in `Suvu::word`/`from_bits`. The code is correct as written - I checked all
three against the rendered Tables 27 and 30 - but the named tests pin these only by round-trip,
and every later package builds on them (bit 29 extends E2u, the flags drive the whole part walk,
SUVu' is defined by the ack bit alone). Suggest a printed-vector assertion for a full CPd word 0
and for one SUVu, as `suvd_vectors_match` already does for SUVd.

**[nit] crates/datapump/src/v92/sequences.rs:434**

`COEFFICIENT_WIDTH` is the only constant in the file whose doc names no clause, against the
house rule that every constant quotes its clause. The width comes from Table 30's 222:237 and
307+alpha:322+alpha rows.

**[nit] crates/datapump/src/v92/sequences.rs:973**

`Finder`, its `keep_from > 4096` retention window and `pad_to` are near-verbatim copies of
`v90::sequences::{Finder, pad_unit}` (the Files line forbids exporting those, so the duplication
is forced), but nothing in either doc says the two mirror each other, so a fix to one will not
reach the other.

**[nit] crates/datapump/src/v92/sequences.rs:754**

`Cpd::check` returns Ok for drn = 0 before looking at the optional parts, so a cleardown CPd
carrying an out-of-range constellation index or an empty set is accepted. DC-7 says a cleardown
*needs* no parts, not that parts it does carry may be nonsense - and 9.11 allows drn = 0 in any
rate sequence, so `merged_over` could keep them.

**[nit] crates/datapump/src/v92/sequences.rs:1052**

`UpFinder::feed` returns `cp.or(short)`, so when a long CP and a CPus/SUVu complete on the same
fed bit the short one is discarded and its finder has already cleared the candidate. Improbable
(needs a false 17-one start inside a CPu payload with an accidentally good CRC) and costs at
most one repetition of a repeating group, but the `.or(...)` reads as though the two could not
coincide.

- Implementer left open: I did not run `git checkout -B wp/V92-11 v92` as the task text literally directs. The worktree
was already on wp/V92-11 carrying d11775f, this package's code commit from an earlier run of the
same workflow, with the plan amendment written but uncommitted - the shape of a run interrupted
at its last step. Resetting would have discarded work that turned out, on review, to be correct.
Instead I verified it against the rendered pages listed under readings before keeping it. If you
would rather it were written from scratch, say so and I will reset the branch and redo it.

- Implementer left open: No file outside the package's Files line was edited, so nothing had to be reported under that
rule. `git diff --stat v92..HEAD` is exactly crates/datapump/src/v92/sequences.rs and
docs/design/v92/plan.md; the tree is clean and nothing was pushed.

- Implementer left open: Two small things in the entry that are wider than what the package built, left as they are
because they are not wrong, only loose. The Q-format bullet lists Q3.13 and Q1.6 among the
formats this package uses; neither appears in sequences.rs, because the fields that use them (Sr
and ld in Table 23) belong to V92-03's CP layout. And SET_POINTS_SENT encodes the plan's "send 2
x LC <= 128, accept LC <= 128" reading of the 128-point limit, which stays open until a capture
settles it (P4D Q3).

- Implementer left open: Nothing here has been near a line. Every vector is either printed in a digest and re-read off
the rendered page, or derived from the CRC of 10.1.2.3.2/V.34 as the existing V.90 code computes
it. The readings section 4 leaves open - the CPd point scale above all - are still guesses with
a documented alternative, and the first real CPd from a V.92 server is what will confirm or
break them.

## V92-12 (wp/V92-12)
- Reviewer verdict: ok; 3 nit, 2 should-fix
- Plan amended: Yes, in its own commit 97bb55a. V92-12's description bullet asked for 'segment-length constants 384, 24, 144, 2040, and UP_SEQUENCE_UNIT = 12', but V92-01 already owns all four in v92/mod.rs as RU_SYMBOLS, RU_BAR_SYMBOLS, SU_SYMBOLS and TRN1U_MINIMUM, and they are in the merged wave-1 code; a second copy would break section 4's 'one reading, one home' rule and is risk R12. The bullet now names wha

**[should-fix] crates/datapump/src/v92/up_signals.rs:907**

`a_two_point_sequence_decodes_whatever_the_line_polarity` never sends E1u, but line 907 says
"CPt, its acknowledged repetition, and E1u behind them" and the test doc at 872-877 cites 8.5.2
("E1u carries on from the CPt"). `cpt_bits` is built at 882-883 as CPt + CPt' only;
`sender.e1u()` is never called on that stream. The plan names this test as round-tripping "Ja,
CPt and E1u ... including the preamble and the differential seed"; E1u is in fact only exercised
by `e1u_is_twelve_zeros_and_is_told_from_another_cpt`, at one TRN1u length and one polarity. A
later package reads the comment and believes E1u's continuation through the differential encoder
is proved at both polarities and both seeds. Adding `sender.e1u()` (and 12 to the expected
length) closes both the comment and the plan bullet.

**[should-fix] crates/datapump/src/v92/up_signals.rs:112**

`TRN1U_RESET_EACH_SEGMENT`'s doc says the alternative reading is "the one line that would
change", but that alternative cannot be produced by this API. Under it the second TRN1u
(9.5.2.1.9, after Ja) must inherit the scrambler that ran through Ja, yet
`TwoPointSender::after(trn1u: Trn1u)` at line 290 consumes the `Trn1u` and nothing returns its
scrambler state, so `v92::up_source` can only build that segment from a fresh zeroed
`Trn1u::new()`. `restart()` is consequently uncallable in the real Ru/TRN1u/Ja/Su/TRN1u/CPt flow
and is used only by `a_second_trn1u_segment_starts_the_same_way_as_the_first`. Setting the
constant false in a scratch copy leaves the second segment's wire behaviour unchanged and breaks
that test, so the claim is false in both directions. Plan section 2 requires one edit to flip an
ambiguous reading: either expose the scrambler (e.g. `Trn1u::after(&TwoPointSender)`) or say in
the doc that the alternative also needs a way to carry state across Ja.

**[nit] crates/datapump/src/v92/up_signals.rs:441**

`TRN2U_ONLY_SIGN_IS_DIFFERENTIAL` is referenced only by its own definition (441) and two
comments (590 inside `Trn2uSender::next_symbol`, 1170 in a test doc). No code reads it, so
flipping it is a no-op and the alternative it documents ("differentially encode all b bits") can
be neither produced nor tested. Compare `TRN2U_SIGN_LAST`, which is a field on both sender and
reader and is asserted at both settings. 8.7.6 is unambiguous, so this is documentation rather
than a switch, but it is the "item nothing references" the plan calls risk R12 and the asymmetry
with the other three section-4 readings will puzzle the next reader.

**[nit] crates/datapump/src/v92/up_signals.rs:239**

The `PREAMBLE_ONES` doc is off by one: "the twenty-fourth symbol is the first whose bit comes
out right". Symbol 1 is the differential reference (no bit) and symbols 2..24 give the 23 bits
that fill GPA's 23-bit register, so the first correctly descrambled bit comes from symbol 25,
the first symbol of the sequence. The code and the test agree -
`twenty_four_ones_let_the_far_descrambler_lock_before_the_sync` asserts `bits.len() ==
PREAMBLE_ONES - 1 + descriptor.len()` and `bits[23..] == descriptor`. As written the sentence
reads as "the preamble yields 24 usable bits", and its own trailing clause ("which is the first
bit of the frame sync") contradicts the count.

**[nit] crates/datapump/src/v92/up_signals.rs:350**

The `TwoPointReader` doc says a far end that followed the TRN1u "does not need the twenty-four
ones at all", in the same paragraph that says nothing upstream fixes an absolute polarity. The
plain (pre-switch) read is polarity-sensitive: `feed_sign` descrambles the received sign
directly, and GPA is linear over its register, so an inverted line descrambles TRN1u's ones to
zeros and leaves 23 inverted bits in the register. After `switch_to_differential` the s(n) are
correct again but the reader needs ~23 more bits to re-lock, losing the start of the first Ja or
CPt - the very case the preamble exists for. The PCM upstream path does not in practice invert,
so this is theory; but the Phase-3 receiver package (V92-22/V92-33) will read the claim as
unconditional. One clause noting the assumption would settle it.

- Implementer left open: Nothing in the package was left undone; all nine plan-named tests exist, plus five more the
package needed. No file outside the Files line was touched (git status proves it), apart from
the amended plan.

- Implementer left open: Naming note for the packages that will call this: the pull methods are next_symbol(), not
next(). A public `pub fn next(&mut self) -> f64` is refused by clippy's should_implement_trait
under -D warnings (the existing v90/v34 sources get away with `next` only because theirs are
private). V92-27's entry names `UpSource::next() -> f64`; if that method is public it will have
to be renamed the same way, or the enclosing type will have to implement Iterator.

- Implementer left open: TRN2U_SIGN_LAST, PREAMBLE_IS_SCRAMBLED and TRN1U_RESET_EACH_SEGMENT are readings, not facts, and
both ends of this project agree whichever way they are set. Only a capture of a real V.92
analogue modem or server settles them; the sender and reader take the bit order as a field so
the_trn2u_bit_order_is_one_switch already tests both settings and a flip stays a one-line
change.

- Implementer left open: The 8-point magnitude bits' own order is folded into the same switch (the whole group's time
order reverses), which is what P4A A2's last sentence asks about; if a capture ever shows the
sign first but the magnitude bits unchanged, that third combination would need a second field.

## V92-13 (wp/V92-13)
- Reviewer verdict: ok; 6 nit, 1 should-fix
- Plan amended: Yes, in a separate commit (abd798d). V92-13's heading changed from (M) to (L) - 2408 tabled octets are about 170 lines of data before any code, and the module came to 1558 lines. Two bullets were added to its Description: AnspcmWatch is Debug but not Clone because v8::ansam::AnswerTone is not (with a note that the fix is one derive in crates/v8/src/ansam.rs, on no package's Files line, and should

**[should-fix] crates/datapump/src/v92/anspcm.rs:1025**

The test quantiser clamps mu-law magnitudes with `magnitude.min(8159)`, one too high: P1D 3.4's
closed form is valid only to m = 8158. quantise(Law::Mu, 8159) computes biased = 8192, chord =
ilog2(8192) - 5 = 8, step = 0, U = 128; `ucode::octet` does `ucode & 0x7f`, so the loudest
possible sample silently encodes as the QUIETEST codeword (0xFF instead of 0x80). Checked
numerically: 8157 -> 127, 8158 -> 127, 8159 -> 128 -> masked to 0. It is unreachable from Table
6 (max |x| = 1887 at scl 1334), so nothing reaches the wire today, but the same off-by-one is
stated as fact in the doc of the PUBLIC constant OVERLOAD_MU at line 439 ("it is the same 8159
the quantiser of 8.3.1 clamps to"), and the quantiser is presented as "the exact G.711 decision
intervals" for later packages to reuse. Fix: `.min(8158)`, and say 8159 is the decision value
the last interval ends at, not a value the quantiser returns.

**[nit] crates/datapump/src/v92/anspcm.rs:334**

Three doc comments print paraphrases inside quotation marks, where house style ("every constant
quotes its clause") makes a quoted string read as verbatim spec text. Line 334: `"a data frame
is 6 symbols" (clause 5, through V.90 5.4)` - V.90 5.4 (rendered p.13) reads "Data frames in the
digital modem have a six-symbol structure." Line 515: `"at least 50 ms" (9.2.1.3, 9.2.3.3)` -
both clauses (rendered pp.49, 50) read "for a minimum of 50 ms". Line 519: `"until ANSpcm is no
longer detected"` - 9.2.1.3 reads "When ANSpcm is no longer detected, the modem shall terminate
TONEq". The readings are all correct; only the quoting is. Every other quoted string in the
module was checked word for word against the rendered page and is exact.

**[nit] crates/datapump/src/v92/anspcm.rs:530**

The TONEQ_STEADY doc describes the fourteen-mark run as "bits 26 to 39, its four one-bits
running straight into the ten ONEs of the next frame", which invites the reading that WXYZ =
1111 is four contiguous bits. Table 2/V.92 puts W at bit 24 and a fixed 0 at bit 25, with X, Y,
Z at 26, 27, 28 (P1A 5.1/5.2), so the four ones before the preamble are X, Y, Z and the frame's
stop bit at bit 29. A later package that trusts the phrase and hunts for WXYZ as a contiguous
field in the QC1a info frame will read the wrong bits. The bit range (26 to 39) and the 14 bits
/ 46.7 ms figures are right.

**[nit] crates/datapump/src/v92/anspcm.rs:11**

The module header says the signals "come in a fixed order, with no gap anywhere in it (Figures 3
to 6)" while the diagram directly beneath it correctly shows the 75 +/- 5 ms silence between
QCA1d/QCA2d and QTS. Figure 3/V.92 (rendered p.46) labels "75 +/- 5 ms  768T  48T", and
9.2.2.1/9.2.4.1 make that silence mandatory. The no-gap rule holds only from QTS onwards; a
reader who takes the sentence at face value drops a silence the Recommendation requires.

**[nit] docs/design/v92/plan.md:2839**

The section 14 note re-sizes V92-13 from M to L and states the real figure (1558 lines)
honestly, but plan section 2 defines L as 800-1300 lines, so 1558 sits outside the band it was
moved into and the note does not reconcile the two. The substance of the amendment is sound: the
2408 tabled octets really are about 170 lines of data the original sizing did not count, and the
file is 1558 lines as claimed.

**[nit] docs/design/v92/plan.md:795**

The amendment correctly records that AnspcmWatch cannot be Clone while `v8::ansam::AnswerTone`
is not, and names V92-49 as the package that will want one inside the Clone
`v90::startup::Analogue`, but leaves `crates/v8/src/ansam.rs` off every Files line including
V92-49's (which lists only v90/startup.rs, v90/server.rs and tests/v92_quick.rs). V92-49 will
therefore hit the same "edited no file outside its own list" rule and have to stop and report -
the loop this amendment was meant to close. The underlying facts all check out: AnswerTone
derives only Debug, its fields (ToneDetector, OnePole, Nco) are all Clone so one derive really
would do it, and v90::startup::Analogue is #[derive(Debug, Clone)].

**[nit] crates/datapump/src/v92/anspcm.rs:783**

QtsWatch::feed returns early forever once `reversal.is_some()`, so the first zero crossing it
ever accepts fixes the downstream frame grid for the rest of the call unless the owner calls
reset(). An owner (V92-49) that arms the watch at the start of the call rather than after
QCA1d/QCA2d would keep any spurious reversal found in earlier line traffic, and the grid handed
to Phase 2 and to Sd in Phase 3 would be wrong with no way to recover. I tried hard to provoke
this and could not: random V.21(L) data, a held 980 Hz mark and a held 1180 Hz space all leave
the watch unlocked with reversal() == None, and a watch fed 2 s of V.21(L) before the real burst
still found the true reversal to 0.002 symbols. So this is a note for the owning package, not a
demonstrated defect - though the margin against a held 1180 Hz is the thinnest of the three (41
deg of phasor turn per period against the 37 deg QTS_STEADY allows).

- Implementer left open: AnspcmWatch is Debug but not Clone, because v8::ansam::AnswerTone derives neither Clone nor
anything that would let its state be rebuilt from outside. v90::startup::Analogue is Clone
today, so V92-49 - which is where the analogue side's short Phase 1 lands - will hit this. The
fix is adding Clone to AnswerTone in crates/v8/src/ansam.rs (every field it holds is already
Clone), which is outside this package's Files line, so it was not made. Recorded in the plan
entry and in section 14. QtsWatch and ToneqDetector are both Clone, so only the combined watch
is affected.

- Implementer left open: QTS_TO_ANSPCM is set to 1 s rather than to anything measured on a real line: the signals leave
only 6 ms between the reversal and ANSpcm, but v8::ansam::AnswerTone's 400 ms envelope means its
verdict arrives a few hundred milliseconds later. Measured at about 310 ms through Network::down
with no delay; a live capture with the project's 1.5 s round trip should confirm the figure is
still generous.

## V92-14 (wp/V92-14)
- Reviewer verdict: ok; 5 nit, 5 should-fix
- Plan amended: Yes, in a separate commit (e97c5f4), three changes. (1) V92-14's entry: the wait-for-CP [MAY] is Table 27 bit 26's own sentence, not a condition 9.6.1.1.2 attaches — 9.6.1.1.2 as printed mentions neither bit 26 nor any option. (2) V92-14's entry: the figure_14 test asks for the figure's shape and the window's own instant rather than its box counts, because the 'RTD + 100 ms' bracket is drawn acros

**[should-fix] crates/datapump/src/v92/exchange.rs:791**

"SUVu 12 symbols long" is not a realisable SUVu length, and the same wrong number is in the plan
and the commit message. The figure_14 doc comment says "with SUVu 12 symbols long that span is 6
ms, not 106"; plan.md:846 repeats it and plan.md:2878 says "an SUVu is 12 or 24 symbols". Table
27 (rendered p.31) pads SUVu's 52 bits to the next multiple of 12 SYMBOLS, and Tables 28/29 give
2 bits/symbol (4-point) or 3 (8-point), so ceil(52/3)=18 -> 24 symbols and ceil(52/2)=26 -> 36
symbols. An SUVu is 24 or 36 symbols and never 12. Both digests state this outright: spec-
phase4-signals-analogue.md:202 "CPus, SUVu | 52 | 72 bits = 36 symbols | 72 bits = 24 symbols"
and spec-phase4-procedures.md:223. The conclusion that Figure 14 is not to scale survives (four
24-symbol SUVu is 12 ms, not 106), but the number reaches V92-39/V92-40 through the comment, and
the same file's figure_12 doc gets it right ("an SUVu of 24 symbols"). The test also scripts
analogue suv = 12: I changed it to 24 in a throwaway copy and the digital side came out [SUVd
x6, CPd, SUVd x75, SUVd', Ed] instead of the figure's five SUVd before CPd, so the box-for-box
fidelity the doc claims currently rests on the unrealisable length rather than on the start
offset.

**[should-fix] crates/datapump/src/v92/exchange.rs:755**

figure_13_a_cpu_heard_first_makes_the_single_cpd_a_cpd_prime does not exercise Table 27 bit 26
at all, although its doc comment and the amended plan both say it does. I changed line 770 to
analogue.flags.wait_for_cp = false in a scratch copy and all 18 tests still passed; setting
digital.cp_ready = 0 with bit 26 still set makes the test fail (only three SUVd before CPd'). So
the whole trace comes from the harness's cp_ready = 120, and the flag is inert. The test's first
sentence ("the analogue modem's SUVu carries Table 27 bit 26, so the digital modem holds its CPd
back") and plan.md:843 ("which is the bit-26 case: the analogue modem asks, and the CPd that
follows the CPu is already CPd'") are therefore both false. The behaviour itself is covered by
a_peer_that_asks_us_to_wait_for_its_cp_delays_our_cp - killing HONOUR_WAIT_FOR_CP and killing
the hold_until expiry each fail only that test - so this is a wrong claim in two doc comments
rather than a gap in the code.

**[should-fix] docs/design/v92/plan.md:255**

The amendment corrected the "9.6.1.1.2 makes it a [MAY]" slip in the V92-14 entry and named
V92-39 and V92-40 as carrying it, but plan section 4's own reading row was left untouched and
carries both the slip and a now-conflicting owner: "a received 1 is honoured only while it costs
nothing, which 9.6.1.1.2 makes a [MAY] | `v92::analogue`, `v92::digital`". This package put
HONOUR_WAIT_FOR_CP in v92::exchange, so section 4's "One reading, one home" rule now points the
two later packages at a different module, which is the duplicate-constant case risk R12 forbids
under -D warnings, and a reader who follows the table lands on the wrong clause.
crates/datapump/src/v92/mod.rs:731 (merged wave-1 code, not this package's to edit) has the same
citation on PeerSuv::wait_for_cp and is not mentioned in the section-14 note either.

**[should-fix] crates/datapump/src/v92/exchange.rs:1092**

leaving_a_silent_period_clears_the_acknowledge_state never sends a CP before silence_ended(), so
most of what it claims to prove is vacuous. need_single_cp, my_cp_end, repeat_cp and settled are
already at their reset values when the call happens, and the assertion
assert_eq!(exchange.my_cp_end(), None) at line 1107 is true before it too. I deleted
`self.need_single_cp = true;` from silence_ended in a scratch copy and all 18 tests still
passed, so the test's own claim that "the one CP is owed once more" is unproven - which is the
half of SILENCE_CLEARS_ACK_STATE that matters for Figures 16 to 18 (rendered p.62 shows plain
SUVd, then CPd, then SUVd' after R-bar-t). Starting a Cp and driving an unacked sequence past
the deadline before silence_ended would close it.

**[should-fix] crates/datapump/src/v92/exchange.rs:337**

Neither effect of acknowledged() is covered by a test. Deleting `self.settled = true;` leaves
all 18 tests passing, and so does deleting `self.repeat_cp = false;`. settled is what implements
9.6.1.1.3's "If the acknowledgement bit is not set in ANY of the CPu or SUVu sequences received
... up to and including ..." (rendered p.51): without it, a run of acked-then-unacked receptions
sets repeat_cp when the first unacked sequence completes past own-CP-end + 100 ms + RTD, and
next_sequence then emits a second CP although an acknowledgement was seen inside the window,
breaking both 9.6.1.1.3 and the "single CPd" of 9.6.1.1.2.
no_second_cp_is_sent_while_acks_arrive only ever feeds acked sequences, so it cannot catch this;
a mixed acked-then-unacked trace would. (The repeat_cp = false half is flagged as inferred in
the module doc, so it is the lesser of the two.)

**[nit] crates/datapump/src/v92/exchange.rs:215**

Two quoted clause fragments are not the Recommendation's words, and one inverts the printed
sense. Lines 215-216 and 1164 quote 9.8.2.1.3 as "after having transmitted an SUVu and received
an SUVd"; rendered p.56 reads "After transmitting an SUVu sequence and receiving an SUVd
sequence" (the same paraphrase is in plan.md:836). Lines 452-453 and 1070 quote 9.8.1.1.2 /
9.8.2.1.3 as "if bit 32 is set in either ... go to"; both clauses read "unless bit 32 is set in
either SUVd or SUVu", i.e. the printed condition is the negation of the quoted one, leaving the
reader to work out that peer_asked_silence() is the negated form.

**[nit] crates/datapump/src/v92/exchange.rs:159**

Context::may_extend_e2u justifies "only in initial training" from P4D Q5 and 8.7.2 - an
inference about upstream frame alignment - when Table 30 bit 29 states it as a [SHALL]. Rendered
p.33: "Extend the length of the E2u sequence: 0 = don't extend; 1 = extend by 1 symbol. This bit
shall be set to zero during rate renegotiation and fast parameter exchange procedures." As
written, a later package may read the restriction as a house choice it can revisit rather than a
printed requirement.

**[nit] crates/datapump/src/v92/exchange.rs:214**

The "one SUV of our own first" gate in cp_due is applied on both sides and in all three
contexts, but is cited only to 9.8.2.1.3, which is the analogue modem's renegotiation entry. The
digital side's entry, 9.8.1.1.2 (rendered p.55), puts no such condition on it: "transmit TRN2d
for up to 16008T followed by SUVd sequences. Upon receiving an SUVu sequence, the digital modem
shall proceed according to 9.6.1.1.2". The gate is still consistent with 9.6.1.1.1's ordering
and is harmless, but the doc should say that 9.8.2.1.3 states it and 9.6.x.1.1's ordering
carries it to the other side, otherwise V92-40 will look for a digital clause that does not
exist.

**[nit] docs/design/v92/plan.md:2882**

The section-14 note says Figure 14's unacknowledged runs come out "68 and 76 instead of 1 and 4"
and writes the digital shape as "SUVd x5, CPd, SUVd, SUVd', Ed". Rendered p.51 draws SUVd x5,
CPd, SUVd x3, SUVd', Ed on the digital line and SUVu x2, CPu, SUVu x4, SUVu', CPu' x3, E2u on
the analogue one, so the figure's corresponding counts are 1 and 3, not 1 and 4. The test itself
is right - it collapses the run and asserts the surrounding shape - only the prose count is
wrong.

**[nit] crates/datapump/src/v92/exchange.rs:1065**

The traces[0] == traces[1] assertion in a_silence_request_in_training_is_ignored proves nothing:
next_sequence's three branches (finished, sent_ack && peer_acked, cp_due) never read silence,
peer_silence or peer_asked_silence in any context, so the two traces would also match in
Renegotiation. The plan does ask for "the trace is identical to the run where the bit was
clear", and the test supplies it, but the load-bearing assertions are the three getters
(mutating defines_silence or request_silence does fail the test). Saying so in the doc comment
would stop a later reader treating the trace comparison as the proof.

- Implementer left open: V92-39 and V92-40 repeat the same mis-citation ('9.6.1.1.2 makes complying a [MAY] in any case',
'which 9.6.1.1.2 allows [MAY]'). Their entries are outside this package, so I left them alone
and named them in the section 14 note instead: whoever builds them should quote Table 27 bit 26
in the code and correct those entries then.

- Implementer left open: The silent-period branch of 9.8 itself (SUVd' until SUVu', Ed, silence, Rt, R-bar-t) is
deliberately not here. This package reports the peer's bit 32 and bit 33 and offers
silence_ended() to clear the acknowledge state, which is what its entry asks for; the branch
belongs to V92-51.

- Implementer left open: Figure 14's printed box counts are not reproducible by a machine that obeys 9.6.2.1.3, for the
scale reason above. The test asserts the run-length shape (SUVu SUVu CPu, SUVu x4, SUVu' x68,
CPu' x3, E2u against SUVd x5, CPd, SUVd x76, SUVd', Ed) plus the repeat instant, rather than the
figure's 1 and 4.

- Implementer left open: Figure 13's exact box count needs the digital modem's CPd to be one boundary late because it is
still being designed from TRN2u. That is the owner's condition (V92-40), so the test scripts it
as a cp_ready sample rather than putting a readiness input into the exchange; next_sequence is
documented as advice an owner that cannot yet send its CP may decline, with the obligation
standing.

## V92-15 (wp/V92-15)
- Reviewer verdict: NOT ok; 5 nit, 6 should-fix
- Plan amended: Yes, in its own commit (46239bc), touching only the V92-15 entry and one note in section 14.

Entry: size M -> L (transmit.rs came to 1026 lines, 560 of them tests); Mode::Interpolated's cutoff is exactly 4 kHz with the reason, and the phase table is interpolated rather than rounded; Mode::Straight is the same kernel at its two phases; the LU constants are named; the network read-back test says it

**[should-fix] crates/datapump/src/v92/transmit.rs:547**

The amended `through_the_network_the_codec_reads_back_the_levels_sent` no longer tests the
reconstruction. With `with_upstream_cutoff(FS/2.0)` = 8000 Hz, `Network::tabulate_up_kernel`
builds `kernel(frac, 0.5, ...)`, which at frac = 0 is an exact delta, so the A/D simply picks
the line sample at the symbol instant. Every interpolator that passes through its points then
reads back exactly, and `at_twice_the_rate_every_other_sample_is_the_level` already asserts
that. Proved by mutation in a scratch copy (C:\Users\Gaming\AppData\Local\Temp\claude\F--
dialupmodem2\88010855-0d3a-43e6-94b3-c299dc9e4360\scratchpad\v92rev15, never in the worktree):
replacing the whole of `kernel_table`'s windowed sinc with a plain linear interpolator `(1.0 -
t).max(0.0)` leaves 13 of the 14 tests passing, including this one,
`a_half_symbol_delay_moves_the_codec_samples_by_half_a_symbol`, `at_twice_the_rate_...`,
`the_reconstruction_carries_a_steady_level_at_every_phase` and
`straight_is_the_interpolated_reconstruction_at_its_two_exact_phases`. Only
`epsilon_resolves_to_a_sixty_five_thousandth_of_a_symbol` notices. The plan's original test is
achievable with a real A/D filter still in the path: I measured the read-back (same test body,
only the cutoff varied) at 3700/3900/4000/5000/6000/8000 Hz as -11.6 / -16.1 / -21.9 / -46.8 /
-49.7 / -305.0 dB for the real kernel and -9.5 / -11.3 / -12.4 / -18.4 / -26.9 / -308.4 dB for
the linear interpolator. At a 5 kHz cutoff the correct transmitter clears the plan's -40 dB
while a wrong one is 28 dB worse, so the test can keep both its filter and its discriminating
power.

**[should-fix] docs/design/v92/plan.md:2884**

The section 14 note's sentence "Widening the filter instead of removing it is worse, because
then our own out-of-band image aliases" is measurably false, and it is the sentence that
justifies removing the filter altogether. Measured read-back with the transmitter as built: 4000
Hz -21.9 dB, 5000 Hz -46.8 dB, 6000 Hz -49.7 dB. Widening is better, not worse, and it is what
lets the plan's own -40 dB be met without making the A/D a sample-picker. The rest of the note
(the half-band cascade argument, the 21.9 dB and 305 dB figures, the L sizing, the exact-4-kHz
cutoff and the interpolated phase table) I confirmed. Every later package reads this note, so
the one wrong claim should go.

**[should-fix] crates/datapump/src/v92/transmit.rs:124**

`KEPT` cites "9.6.1.2.2" for the CP repeat window of 100 ms plus a round-trip delay. That clause
does not exist: 9.6.1.2 (Recovery procedures, digital modem) has only 9.6.1.2.1, the 20 s + 6
RTD B1u timer. The 100 ms plus a round-trip delay is 9.6.2.1.3 (V.92 page 52, "...received after
100 ms plus a round-trip delay from the end of its CPu").

**[should-fix] crates/datapump/src/v92/transmit.rs:869**

`symbol_at_says_when_a_symbol_reached_the_line` says "the turnaround 9.4.1.1.2 asks for is
measurable here". 9.4.1.1.2 is the digital modem conditioning its receiver to detect Tone A. The
40 +/- 1 ms at the line terminals is 9.4.2.1.3 (V.92 page 45, the only occurrence of "40 +/- 1
ms" in the Recommendation): "...the time duration between receiving the Tone B phase reversal at
the line terminals and the appearance of the Tone A phase reversal at the line terminals is 40
+/- 1 ms".

**[should-fix] crates/datapump/src/v92/transmit.rs:92**

`CUTOFF` says "6.5's note applies: V.92 specifies no transmit mask for PCM upstream" and `PEAK`
(line 137) says "6.5 specifies no transmit mask at all". V.92 has no clause 6.5: clause 6 runs
6.1 Data signalling rates, 6.2 Symbol rate, 6.3 Scrambler, 6.4 Transmitter with 6.4.1-6.4.4,
then clause 7 (contents page and PDF pages 10-14). "6.5" is the INTRO digest's own section
heading "Items clause 6 does not specify". As written both docs say a clause states something
when in fact no clause states anything, and the next reader will go looking for it.

**[should-fix] crates/datapump/src/v92/transmit.rs:138**

`PEAK`'s provenance is for the other direction. It says "it comes from a live call: through a
softphone everything to about a third of full scale arrived exactly, and everything much above
it was held down by something with a gain control which then read low for a third of a second
(memory v90-live-test-pending)", and the module doc (line 69) states as fact that "the softphone
capture path has a limiter of its own". The memory records that gain control on the DOWNSTREAM,
on codewords BinModem received ("codewords up to about 0.32 of full scale were exact; louder
ones were held to about 0.6 and read low for about 0.3 s afterwards"), and neither that memory
nor crazytel-pcm-path records anything about the capture path. Applying the same ceiling
upstream is a sound and conservative inference, and the plan asks for it, but it should be
written as an inference so a live test can falsify it.

**[nit] crates/datapump/src/v92/transmit.rs:168**

Plan section 4 owns the reading "S-bar-u 24.5T and (24+eps)T = a lasting delay of every later
upstream symbol, not extra symbols" to `v92::transmit`, and requires the owning constant's doc
to give the clause "and, where the reading is open, the alternative". P3S A1 marks it "Open;
confirm with a capture" and names three alternatives (hold the last value, insert zero, shift
time). `SU_BAR_HALF`'s doc gives the clause and `delay`'s doc argues for the shift, but neither
names the alternatives nor says the reading is open, so a capture that disagreed would not find
the switch documented. (The reconstruction in fact fills the 0.5 T gap with the band-limited
interpolation between the neighbouring symbols, which is a fourth answer and worth saying.)

**[nit] docs/design/v92/plan.md:164**

AD-8 still describes `Mode::Interpolated` as a "windowed-sinc reconstruction under 4 kHz", which
the V92-15 entry and `CUTOFF` now contradict (exactly BAUD/2, because only the symbol rate's own
Nyquist has its zeros on the other symbols' instants). The amendment rule only asks for the
package entry plus one section 14 note, so this is within the letter of the rule, but AD-8 is
what a reader meets first.

**[nit] crates/datapump/src/v92/transmit.rs:94**

American spellings against the British-spelling house rule: "equalizes" here and "equalizing" at
line 542. The repo uses "equalis-" 54 times across v34 and v90 and no "equaliz-"; the file
itself uses "quantise" at line 111.

**[nit] crates/datapump/src/v92/transmit.rs:116**

`LEVELS`'s doc says "the taps it reaches, and one either side so a pull never has to look at one
that has gone", but `LEVELS = 2 * REACH + 2` is 41 taps plus one spare in total, not one each
side. The buffer is in fact sufficient (after the placement loop the newest symbol is in (t+19p,
t+20p], so 42 symbols reach back to t-22p), but the comment describes 43.

**[nit] crates/datapump/src/v92/transmit.rs:120**

`KEPT` = 2 s of instants is sized from "a round trip on this project's lines is about 1.5 s",
but voip-line-round-trip closes with "Any timeout or patience constant has to hold up at three
seconds of round trip". At 3 s RTD the 100 ms + RTD window falls outside the kept instants and
`symbol_at` silently extrapolates at the current rate. The degradation is graceful (the
extrapolation is anchored on a real stored instant, so the error is only the rate change since),
so this is only worth a sentence in the doc or a larger KEPT.

- Implementer left open: No file outside the package's Files line was edited (git diff --stat over the two commits shows
only transmit.rs and plan.md).

- Implementer left open: the_transmitter_follows_a_network_120_ppm_fast drives the transmitter from a synthetic
SymbolClock carrying the true rate, not from a trained pcm::Receiver. Building a real one here
would have meant duplicating pcm.rs's private phase3_levels helper and a ten-second training run
for a question V92-06 already answers
(an_upstream_timed_by_the_symbol_clock_keeps_its_phase_for_ten_seconds, 0.05 T). Recorded in the
amended entry. If a later package wants the two halves joined, v92_upstream.rs is the place.

- Implementer left open: CREST = 3.0 has no basis in the Recommendation - V.92 names no transmit mask (6.5) and no
ceiling on LU (3.8). It is a crest factor chosen so Su (1.22 LU, 8.5.6) fits with better than
half the room to spare for the precoder, whose peaks nobody can predict because x(n) is never
saturated. The alternative, 2, is in the doc. A live capture of what the softphone capture path
actually limits at would settle it; that is section 13's business.

- Implementer left open: Mode::Straight's behaviour away from its two exact phases (snap to the nearer) is a choice, not
a requirement. It means a Straight-mode transmitter following a skewed clock walks across the
two phases in half-sample steps rather than sliding. That is arguably what a verbatim-forwarding
path really does to a clock offset, but V92-17's UpPath::Straight is where the network side of
that question lives, and only a capture settles which mode a live line wants (AD-8).

- Implementer left open: Nothing calls PcmTransmitter yet: v92::up_source and v92::analogue are still wave-1 stubs, so
the transmitter is exercised only by its own tests and Network. The first caller is V92-41.

## V92-16 (wp/V92-16)
- Reviewer verdict: NOT ok; 3 nit, 4 should-fix
- Plan amended: Yes, in its own commit aa95a27, touching only docs/design/v92/plan.md. The V92-16 entry was corrected in three places and section 14 gained one note. (1) The entry's claim that `training_codeword` "can return a Ucode above 111", and its named test "against a digital modem whose INFO0d maximum transmit power would otherwise admit Ucode 120", are both false against Table 15/V.90: the loudest limit a

**[should-fix] crates/datapump/src/v34/phase2.rs:904**

A V.92 analogue modem reads a plain V.34 call modem's transmit clock source as the V.92 flags.
The `Info::Info0` arm writes `far_flags` whenever `self.pcm.is_some() &&
self.far_info0d.is_none()`, and its own comment says it is "for the INFO0a a digital modem
hears" — but an analogue PCM modem (Role::Answer, receiver on Side::Call) reaches it too when
the far end is a V.34 call modem sending INFO0c. Table 14/V.34 (rendered p.34) gives bits 26:27
as 0=internal, 1=synchronized to receive timing, 2=external, 3=reserved, so the very common
clock=1 sets `far_flags.v92`. `both_v92()` and `lapm_bypass_allowed(true)` then return true
against a modem that has never heard of V.92; with clock=3 `short_phase2_agreed()` does too. The
scenario is already exercised by the existing test
`a_v90_analogue_modem_meeting_v34_asks_for_v34`; I proved the misreading in a throwaway copy
under my scratch folder with a probe test asserting `!analogue.both_v92()`, which fails on this
branch. Nothing reaches the wire today because Table 18 additionally requires `far_info0d`, but
V92-24 and the error-control package read exactly these accessors. Unlike the digital-side
reading section 14 documents as undefendable, this direction is free to close: the analogue
modem's far flags always come from the `Info0d` arm, so this write only ever needs to fire for
`Role::Call`.

**[should-fix] crates/datapump/src/v90/mod.rs:105**

The UINFO cap the package exists to add is not covered by any test.
`uinfo_never_exceeds_111_so_sd_s_codeword_exists` exercises the private `loudest_codeword` at
its ceiling, never `training_codeword_v92`. Because Table 15/V.90's loudest limit (15124)^2
already caps the search at Ucode 109 under both laws, the equality assertion against
`training_codeword` passes whatever ceiling `training_codeword_v92` passes. Proved by mutation:
replacing `V92_MAX_UINFO` with `(ucode::UCODES - 1) as u8` inside `training_codeword_v92` in a
throwaway copy leaves `cargo test -p datapump --release` at 364 passed, 0 failed, the named test
included. A later edit that drops the cap from the public function — the exact regression the
constant exists to prevent — would land silently. One assertion tying `training_codeword_v92` to
`V92_MAX_UINFO` for every INFO0d would close it.

**[should-fix] docs/design/v92/plan.md:2903**

The section 14 note handed to V92-24 misreads V.34's clock encoding: "a V.34 modem naming an
external clock (the value 3) sets *both* flags". Table 14/V.34 (rendered p.34) gives 2 =
external and 3 = reserved for ITU-T. With `Info0::pcm_flags` reading v92 = clock&1 and
short_phase2 = clock&2, no conforming V.34 modem can set both flags: clock 1 sets only the
capability, clock 2 (the real external clock) only the short-phase-2 request, and the both-flags
case is the reserved value. `v34/info.rs`'s own `FLAGS_SHIFT` doc has it right ("the value 2"),
so the two documents now contradict each other on a number read from a table. The note's
conclusion — gate the short-phase-2 request on V.8 having offered PCM — survives; the stated
hazard behind it does not.

**[should-fix] crates/datapump/src/v34/phase2.rs:973**

Digital acceptance of a Table 18 INFO1a checks both-V.92 and bit 70 but not
`asked.uinfo_is_sendable()`, so a U_INFO of 0 or 120 is taken as a good frame.
`Info1aPcmUp::from_bits` deliberately parses any 7-bit value, so a far end naming U_INFO 120
sets `Status::Done` and hands `info1a_pcm_up()` to Phase 3, where Sd is "the PCM codeword whose
Ucode is 16 + U_INFO" (8.4.4/V.90, kept by 8.6.7/V.92) — Ucode 136, which does not exist among
the 128 there are. P2S N-22 names this case and suggests counting the frame as not received,
which is the recovery this arm already implements for its other two conditions, and
`uinfo_is_sendable()` is already written. The plan entry does not name it, so this is an
omission rather than a deviation, but V92-16 owns digital acceptance and a later package will
index past Table 1 with it.

**[nit] crates/datapump/src/v34/phase2.rs:1250**

`table_18_allowed`'s doc reads as a disjunction where the code is a conjunction. Under "Four
things, and all of them", the first bullet is "both modems have shown V.92, or the digital
modem's INFO1d is Table 9 and its bit 70 means a carrier rather than a channel". The "or" is
meant as the reason for the condition, but a later package reading the bullet list as the
predicate's shape would conclude Table 18 is permitted against a V.90 Table 9 INFO1d, which is
the opposite of 9.3.

**[nit] crates/datapump/src/v34/phase2.rs:656**

`refused_info1a` is never cleared when a later INFO1a is accepted, so its doc ("Why the last
INFO1a was counted as not received") is not what it returns. A Table 18 frame that arrives
before this end's INFO1d has gone is refused; the far end repeats it after the INFO1d and it is
accepted; `status()` is `Done` but `refused_info1a()` still reports the stale reason.

**[nit] crates/datapump/src/v34/phase2.rs:981**

The refusal reason blames the far end for this end's capability: the `else` branch reports "a
table 18 info1a from a far end that did not indicate v.92" whenever `both_v92()` is false,
including when it is false because this end is not V.92. Reachable for a plain `Pcm::Digital`
modem, because `dpsk::classify` now tries `Info1aPcmUp::from_bits` for every Answer-side 70-bit
frame regardless of version.

- Implementer left open: One reading could not be settled and is recorded in plan section 14 rather than coded round: a
V.92 digital modem cannot tell INFO0a bit 26 from V.34's transmit clock source. V.90 reserves
bits 26:27 and a V.90 analogue modem sends zeros, so V.90 peers are safe, but a plain V.34 modem
answering a V.92 digital modem's Phase 2 puts its clock there, and 'synchronized to the receive
timing' (1) reads as 'V.92 capability: 1'. 9.3 offers no second signal and Phase 2 cannot see
the V.8 menus. The cost here is one bit — INFO1d bit 70 carries the PCM-upstream verdict where
V.90 would put the 3429 high-carrier flag — and the call still lands on V.34;
`a_v34_info0_with_a_clock_of_1_is_not_taken_for_v92` pins exactly that rather than pretending
otherwise.

- Implementer left open: A sharper form of the same thing is left for V92-24: a V.34 modem naming an external transmit
clock (the value 3) sets both flags, so if our own end also requests a short Phase 2 the four
bits of 9.4 would agree with a modem that has never heard of it, and the two ends would reverse
in opposite orders. Nothing in this package can request a short Phase 2 on a live call
(`V92Wish` is only constructed in tests, and `v90/startup.rs` still builds the V.90 variants),
so there is no live exposure today. The remedy belongs with the request, not with Phase 2: gate
it on V.8 having offered PCM and on AD-12's memo, as P2S N-21 suggests.

- Implementer left open: `short_phase2_agreed()` exists and is tested but nothing acts on it: short Phase 2's own stages
are V92-24. `lapm_bypass_allowed()` likewise has no consumer yet; the error-control layer takes
it later.

- Implementer left open: `v90/startup.rs` is untouched, so a Table 18 INFO1a leaves `info1a_pcm()` empty and no hand-over
happens — the V.92 analogue and digital modems are V92-29's. Nothing in the shipped modem can
reach the new `Pcm` variants yet, which is why every existing V.90 call is byte-identical.

## V92-17 (wp/V92-17)
- Reviewer verdict: NOT ok; 6 nit, 4 should-fix
- Plan amended: No. The entry could be followed as written, so plan.md was not touched. Two things a later package should know are documented in the code rather than in the plan: `UpPath::Straight`/`Resampled` lag by one packet (the far jitter buffer's standing depth, on `prime_codec` and `with_up_path`), and `with_transcoder`/`with_far_echo` gained `_in(dir, ..)` siblings following V92-04's convention, since the

**[should-fix] crates/datapump/src/v90/network.rs:992**

The downstream echoes itself at twice the hybrid loss. down() at line 896 deliberately remembers
the pre-echo codeword in sent_down to avoid a loop on that side, but up() at line 992 remembers
`out`, which already contains self.up.echo_of(&self.sent_down) from line 979 plus the quantiser.
Measured: Network::new(Law::Mu, 16000).with_upstream_gain(1.0).unquantised().with_echo(20.0,
&[1.0]), one codeword of ucode::level(Mu,100) sent downstream, up(&[0.0,0.0]) every tick so the
analogue modem says nothing at all. Differenced against the same route with no echo, the
analogue modem hears a copy of its own downstream codeword one codeword later at 0.0030047 =
-40.4 dB, exactly gain^2 (2 x 20 dB). A hybrid reflects what arrives on its 2-wire side, which
is the analogue modem's signal only; the downstream folding back into the downstream is an
artefact. At a realistic 10 dB ERL it is -20 dB on a downstream that needs ~50 dB, so V92-26's
echoed M0 cases and V92-44's canceller would see the downstream rate collapse for a reason that
is not an echo canceller problem. Replacing line 992 with `self.remember(Direction::Up, if
self.quantised { quantise(self.law, heard) } else { heard })` drives the difference to exactly
0.0 and leaves all 21 network tests green, so nothing pins the present behaviour either way. Not
on the wire today, because the default route has no echo.

**[should-fix] crates/datapump/src/v90/network.rs:1621**

Nothing tests that the hybrid echo lands before the quantiser, which the plan's V92-17 entry
puts in bold and the module doc at line 635 calls 'the reason a canceller can never quite undo
it'. Both halves of the_hybrid_echo_is_where_it_was_put use .unquantised(). Proof: replacing
`self.carry_up(sum)` at line 984 with `self.carry_up(heard) +
self.up.echo_of(&self.sent_down).unwrap_or(0.0)`, i.e. the leak arriving after the codec instead
of before it, leaves all 21 network tests and the whole workspace green. V92-44's canceller is
built on exactly that quantisation residual. A quantised variant of the impulse case, asserting
the echo lands on the codeword grid rather than on the level, would pin it.

**[should-fix] crates/datapump/src/v90/network.rs:640**

with_echo's peak normalisation is never exercised. The doc promises the taps are scaled so the
largest of them is hybrid_db down, but the only echo tests use the single-tap shape
[0.0,0.0,0.0,1.0], where max, sum and L2 norm all equal 1. Proof: replacing `let peak =
taps.iter().fold(0f64, |peak, t| peak.max(t.abs()));` with `let peak: f64 = taps.iter().map(|t|
t.abs()).sum();` leaves all 21 tests and the whole workspace green. Any caller passing a multi-
tap shape, which is the point of the `taps: &[f64]` parameter, would get an echo at the wrong
level with nothing to say so, and plan section 5 lists V92-17 as a dependency of V92-26
precisely for 'the echoed cases in the M0 sweep', which set an ERL in dB and read a cancellation
figure back in dB. A test with e.g. [0.5, 1.0, 0.25] asserting the peak tap lands at exactly
hybrid_db would close it.

**[should-fix] crates/datapump/src/v90/network.rs:1780**

a_resampled_path_does_not asserts only negatives (fewer than 15 exact codewords, less than half
the power back), so a resampled path that carries nothing passes it. Proof: making
Resampled::feed return immediately, so the chain produces nothing and every up() falls through
`self.to_codec.pop_front().unwrap_or(0.0)` to zero, leaves all 21 tests green (exact 0 < 15, got
0 < 0.5 * sent). The plan pairs this test with
a_straight_softphone_path_hands_the_encoder_our_samples_exactly to answer 'which of those it is
decides whether PCM upstream is possible at all', and V92-26's go/no-go gate reads that answer,
so a silent path would answer it the same way for the wrong reason. The real path is healthy
(instrumented: to_codec steady at 138 codewords over 80 000 samples, no underrun, rms 0.124
out), so a positive assertion is cheap: the DC half of the level,0 alternation should survive at
about a quarter of the sent power, not at nothing.

**[nit] crates/datapump/src/v90/network.rs:958**

The upstream limiter's position relative to up_gain is untested. It is applied to the raw line
sample before up_gain, which matches its doc ('the analogue modem's own samples, before they
reach the codec'), but both tests that touch it use with_upstream_gain(1.0). Proof: rewriting
lines 958-960 as `let limited = self.up.limit(self.up_gain * x, self.fs);
self.take_sample(limited + noise);` leaves all 21 tests green. The ceiling is a fraction of full
scale, so the two placements differ as soon as up_gain is not 1.0, and digest CT section 7 item
14 plans to sweep up_gain at 0.25, 0.5 and 1.0. One case at 0.25 would fix the meaning.

**[nit] crates/datapump/src/v90/network.rs:116**

CRAZYTEL_LOW_PASS's doc arithmetic does not reproduce the constant on the next line. Worked
through as written, 250/(0.769-0.363) = 615.76 and start = 3750 - 0.363*615.76 = 3526.48, giving
(3526.48, 4142.24) and responses of -2.9918 dB and -17.9936 dB rather than -3 and -18. The
constant itself is the right one (unrounded fractions 0.3634696 and 0.7690888 give 3525.9786 and
4142.3202), so only the prose is out; either print more digits or say the fractions are rounded.

**[nit] crates/datapump/src/v90/network.rs:98**

SOFTPHONE_FS = 48_000.0 is the one new constant with neither a clause nor a cited source,
against plan section 2's rule that every constant quotes its clause. The 48 kHz, the
raised/lowered names and the delay queue come from the existing softphone model at
crates/modem/tests/v90_call.rs:214-216, which digest CT 2.5 describes and which CT section 7
item 9 asks to move into the network. One line naming that Pair would settle whether 48 000 is a
measurement, a guess or an existing model's number, as CRAZYTEL_LOW_PASS's doc does for its own.

**[nit] crates/datapump/src/v90/network.rs:1084**

clock_slip fires on every non-Loop path, but the plan's V92-17 entry says 'On a Straight path a
clock offset becomes upstream slips at 20 ms / |delta|, not skew' and digest CT section 7 item
10 says the same. On UpPath::Resampled a clock offset now produces both a slip train and a
resampler chain built at the unskewed fs. The widening is defensible (a resampling softphone
still forwards at its own packet rate) and the method doc says 'on a softphone path', but it is
silent against the plan: either say in the doc why Resampled is included, or amend the plan
entry to say 'softphone path'.

**[nit] crates/datapump/src/v90/network.rs:326**

On a transcoded leg up_code() is not a codeword of either law. After the gateway re-encodes into
`to`, the Ucode is recomputed with the route's `law` against a level that now sits on the
gateway's grid, although the doc at line 300 calls it 'the one the far end reads'. With
with_transcoder_in(Direction::Up, Law::A, None) on a mu-law route, up() returns an A-law level
(a_transcoded_leg_re_encodes_in_the_gateways_law asserts exactly that) while up_code() reports
the nearest mu-law Ucode to it. Nothing asserts up_code on a transcoded leg, so a later V.92
upstream receiver test written against it would be reading a fabricated codeword.

**[nit] crates/datapump/src/v90/network.rs:641**

with_echo silently drops all-zero or empty taps: peak is 0, so shape becomes Vec::new() rather
than the zero-valued filter of the length asked for, and the Echo is still kept as Some.
echo_reach() is then 0 so nothing is remembered and echo_of() returns Some(0.0). Harmless today,
but the length the caller asked for is not what the model holds. Keeping the zero-valued shape,
or returning self unchanged, would be honest about which it did.

- Implementer left open: Nothing in the package was left out. Two deliberate limits, both documented at the code: a lost
stretch on a softphone path needs a packet of standing buffer plus the upstream leg's delay as
slack, because the model's caller hands `up()` exactly one codeword's worth of samples per tick
and so never produces the surplus a fast clock really would -- a route with no upstream delay
can therefore lose one stretch and then has nothing left, exactly as a loop with no delay cannot
read ahead; and `UpPath::Straight` assumes `fs` is a multiple of 8000, since it takes one line
sample in `fs / 8000`.

- Implementer left open: The upstream leg's echo source is what the downstream leg carried, not the reconstructed D/A
waveform, and the downstream leg's is the A/D's output rather than a loop reflection. Both run
at the network rate in the level domain, which is one mechanism instead of two at different
rates; the cost is that the analogue modem's own echo arrives band-limited to the A/D's edge.
`with_echo`'s doc says so.

## V92-18 (wp/V92-18)
- Reviewer verdict: NOT ok; 2 nit, 4 should-fix
- Plan amended: Yes, in its own commit (0b7af22). V92-18's section 5 entry: the Clauses line gained 8.2.2, 8.2.5, 8.3.1, 8.3.5, 8.3.6 and 9.2/9.2.1.3/9.2.4.2; the Digests line gained P1P; the Description gained a "What it turned out to hold" paragraph; the test list replaced `the_v92_menus_offer_pcm` (which has no answer in this capture) with `the_v8_menus_never_come_because_this_call_used_short_phase_1` and adde

**[should-fix] crates/datapump/tests/v92_vector.rs:561**

The "93 ms where 9.2.1.3 asks for 75 +/- 5" departure is a measurement artefact, not a real
modem's behaviour. `silence = info0a.0 - 49.0/600.0 - toneq_off` assumes `v34::dpsk::Receiver`
reports an INFO0 exactly 49 bits after the burst starts. It does not: `feed` yields "the moment
its CRC checks", i.e. after 45 of the 49 bits (the trailing FILL is never required, dpsk.rs
`decide`), the burst is preceded by V.34's arbitrary reference-point symbol, and the receiver
adds its own group delay (401-tap channel select = 12.5 ms plus a 161-tap matched filter = 5
ms). Measured end to end with this repo's own Transmitter+Receiver at fs = 16000, first non-zero
line sample to report is 1628 samples = 101.75 ms, identical for both Sides. Cross-checked
against the capture: INFO0d is reported at 3.506 s and 101.75 + 13/600 = 123.4 ms earlier is
3.382 s, exactly where the 1200 Hz band rises; INFO0a is reported at 3.502 s, so its carrier
starts at 3.400 s, exactly where the 2400 Hz band rises. TONEq's strong 980 Hz stops at ~3.321
s, so the silence is about 79 ms - inside 75 +/- 5, and inside the +/-6 ms window resolution.
The wrong figure is asserted in the range at line 563, narrated in the module doc (lines 33-35)
and the test (551-559), in the commit message, in plan.md line 988, and in plan.md section 14
lines 2912-2916 as this package's one "measured departure", where a later Phase 1 package will
read it as evidence that real modems overrun the clause.

**[should-fix] crates/datapump/tests/v92_vector.rs:20**

"The CRe that opened it, and the first part of QC2a, are before the clip starts" is false, and
so is plan.md line 2888 and the "from 1.1 s to 3.4 s ... the second half of Figure 5" of line
2886. The file is exact digital silence to 0.70 s; CRe segment 1 (1375 + 2002 Hz) runs 0.775 to
1.055 s - 280 ms, the shortened V.8 bis form - and segment 2 (400 Hz) runs 1.055 to ~1.155 s
(100 ms). QC2a's V.21(H) mark preamble then starts at ~1.18 s and its first HDLC flag falls at
1.301 s, so the 100 ms +/- 2% preamble of V.8 bis 7.2.4 is entirely present and measurable. The
comment at line 434 ("The clip opens part-way through its 100 ms mark preamble, so the frame
itself is all that can be checked") confuses the test's own 1.0 s window with the clip, and a
requirement the package could have pinned is written off. The cost is real: section 14 now tells
a future V.8 bis package that it has no CRe to check a detector against, when it has a clean
one.

**[should-fix] crates/datapump/tests/v92_vector.rs:673**

The ignored Ru test's control does not show what it claims. `assert!(period_6(2.0,
2.2).is_some(), "the detector cannot even find QTS")` passes on QTS, but the 4 kHz gate the
detector applies is four times easier for QTS than for Ru. For QTS {+V,+0,+V,-V,-0,-V} the
6-point DFT gives |X1| = 2V at 8000/6 and |X3| = 4V at 4000, so at the codec 4 kHz is twice the
fundamental; for Ru {+LU,+LU,+LU,-LU,-LU,-LU} (8.5.5) it is |X1| = 4LU and |X3| = 2LU, so 4 kHz
is half the fundamental. FOUR_KHZ_PRESENT = 0.002 was calibrated on QTS, whose 4 kHz measures
0.0045 against a 0.0207 fundamental; an Ru of the same fundamental strength would leave only
~0.001 at 4 kHz and be rejected. So the comments at lines 56-58 ("One detector therefore finds
either of them ... QTS proves the detector works") and 665-670 overclaim, and the day a capture
does carry an upstream this hunt may silently find nothing.

**[should-fix] docs/design/v92/plan.md:2902**

"Every printed number that reaches the wire in short Phase 1 is now confirmed by a real modem"
is an overclaim, and it hides the one check the capture most wants. The package pins framing and
timings only; it reads U_QTS = Ucode 79 out of QC2a and LM = 01 out of QCA2d and then never
checks that they reach the waveform - nothing would fail if the digital modem had used a
different codeword for QTS's V (8.3.6) or a different ANSpcm level (Table 6), which is what
those two fields are for. The check is cheap and channel-independent, because both signals come
from the same end back to back. Ucode n is mu-law codeword index n, so Ucode 79 has G.711
magnitude ((2*15+33)<<4)-33 = 975 (the digests' V.90 Table 1 "linear" column is 4x this: 975*4 =
3900, 439*4 = 1756 for Ucode 61, 1471*4 = 5884 for Ucode 87). QTS's 8000/6 component is (4/6)*V
= 650; ANSpcm at -12 dBm0 has scl = 1000 and the rendered Table 6 equation is scl * sqrt(2) *
cos(...) - sqrt(2), not the "2" the lossy text extract shows - so its amplitude is 1414.
Predicted ratio 0.4596 (-6.75 dB); measured on this tap 0.02067/0.04548 = 0.4545 (-6.85 dB). It
holds to 0.1 dB and is not asserted anywhere.

**[nit] crates/datapump/tests/v92_vector.rs:52**

`F_FRAME_3RD`'s doc, "The third harmonic of the 8 kHz frame rate, 8000/6", inverts the
relationship. 8000/6 Hz is the fundamental of the six-symbol pattern and 4000 Hz is its third
harmonic - which is what the FOUR_KHZ_PRESENT doc at line 84 says ("the six-symbol pattern's
third harmonic is twice its fundamental"). 8000/6 is the sixth sub-multiple of the symbol rate
under every reading, so the constant's name and its first line will mislead the next package
that borrows the detector.

**[nit] crates/datapump/tests/v92_vector.rs:416**

The comment says the one V.92 synchronisation pattern on V.21(H) sits "where the identification
octets and the frame check happen to alternate". It is wholly inside the frame check sequence.
The frame on the line is two flags, 2D 25, FCS A3 EA (confirmed both by recomputing the CRC and
by demodulating the recording), so the information field is frame bits 16:31 and the FCS is
32:47; the only 0101010101 run starts at bit 36 and ends at bit 45, with 1001001100 in front of
it. The assertion and the conclusion about BitWatcher are right; only the stated reason is
wrong.

- Implementer left open: No V.8 bis quick-connect codec was added. `crates/v8/src/quick.rs` covers only the four
V.8-framed sequences (QC1a, QCA1a, QC1d, QCA1d) and says so; the only real V.92 capture uses the
V.8 bis pair QC2a/QCA2d instead. The two identification octets are unpicked inside the test with
`ec::hdlc`, which is enough to read the capture but is not a codec anything could transmit.
`crates/v8/src/quick.rs` is outside this package's Files line, so adding `Qc2`/`Qca2` and their
V.8 bis framing is a separate package's job - and section 13's live tests are the only thing
that can say whether a V.92 far end will ever offer short Phase 1 to us.

- Implementer left open: `tests/vectors/README.md` still calls this file "ITU-T V.92 56k, V.34-style startup", which is
wrong about the opening - it is a V.8 bis short Phase 1. The README is outside the Files line,
so it was left alone; the correction is recorded in the test's module comment and in plan
section 14 instead.

- Implementer left open: The clip begins part-way through QC2a's mark preamble, so the CRe that opened the transaction
and the first ~200 ms of QC2a are not in the recording and nothing here can check them. The V.8
bis preamble length (100 ms +/- 2% of mark) is therefore unverified.

- Implementer left open: The QTS level was not checked against U_QTS = Ucode 79. Table 2's codes differ by about 3%
between neighbours, which the line's own frequency response swamps, so nothing in the capture
can say whether the server obeyed the exact codeword it was asked for.
