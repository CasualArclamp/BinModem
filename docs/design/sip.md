# The SIP line

## Why this crate exists

`architecture.md` lists the line side as "Sound card into VB-Cable into a
softphone | Local softswitch for development; an outbound SIP trunk to reach
real answering modems later". This crate is that "later" arriving. It places
the call itself, so the path from the modem to the network is:

```
before:  modem -> sound card -> VB-Audio virtual cable -> MicroSIP -> network
after:   modem -> sip::Line -> network
```

The middle stretch that comes out is not neutral. Three things in it actively
damage a modem signal, and all three have already cost debugging time on this
project.

**The softphone's adaptive jitter buffer inserts audio.** Measured on live V.34
captures through MicroSIP: the far modem's signal jumps forward by 20.0 ms with
a fade across the join, every few seconds. One such insert put the QAM receiver
out of lock for 1.3 s. The same seams made a line that measures 43 dB signal to
noise measure 17 dB, because the estimator was measuring the joins and not the
line. Nothing downstream can defend against this: by the time the samples reach
the modem they are indistinguishable from a line whose clock moved.

**Rate conversion destroys codewords.** The softphone converts 48 kHz to 8 kHz
and back. A V.90 server chooses its output *from the G.711 alphabet* -- the
levels it sends are codewords, not samples that happen to be companded -- so a
receiver that gets the codewords back unaltered is reading the far end's own
symbols. Measured: a call through one provider trained at only about 37 dB
because the path decoded mu-law, low-passed it and re-encoded it as A-law,
while a path that delivered exact codewords arrived bit-exact and measured
82 dB. No equaliser can undo a requantisation.

**Gain control**, which a modem never wants and every softphone applies.

Taking the softphone out removes all three, and removes some of the roughly
750 ms one-way delay this rig carries as well -- every stage in that chain was
a buffer.

## Decisions

| Area | Decision | Rationale |
|---|---|---|
| Transport | UDP or TCP, chosen per account; no TLS | UDP is the default and what a trunk expects: a datagram is a message, and the protocol retransmits because nothing under it will. TCP is there for a trunk that insists on it and for the INVITE that outgrows the path MTU, where a fragmented datagram is dropped by more routers than one would like. TLS is left out because it hides nothing -- the call is plaintext at the far end anyway -- and would make every capture unreadable |
| Codecs | G.711 mu-law and A-law, and nothing else, ever | Every other codec in a softphone's list is a speech coder. It fits a model of a voice tract and transmits the model, and a modem signal does not survive the fitting. An answer of G.729 is not a worse call, it is a call that cannot carry one bit |
| Jitter buffer | Fixed depth, silence concealment, everything counted | See below. The adaptive buffer is the fault this crate was written to remove |
| Transmit clock | Ours, 20 ms, always running | A far end applying silence suppression must not be able to stop our transmission |
| Far end address | Symmetric RTP: follow where the audio came from | The address in the far end's SDP is the address it believes it has, which behind a router is not where its packets come from |
| RTCP | Not sent, but recognised | A report decoded as G.711 is a burst of loud noise into the modem |
| Dependencies | `dsp` for the rate conversion, and nothing else | MD5, the random tokens, the URI and message parsing and the G.711 tables are all fully specified in their documents and are shorter than the argument about which crate to take |
| Account file | Plain text, at `%APPDATA%\BinModem\sip.txt`, written as well as read | A trunk that will not register is exactly when somebody has to read the file to find out why. The credentials window writes it through `Account::save_all`, which parses what it is about to write before writing it |
| Concurrency | One call at a time; one thread owns the link | The agent has at most four client transactions outstanding -- a registration, an INVITE, a CANCEL and a BYE -- so they are four fields rather than a table. What cannot be skipped with them is the matching: a response is checked against the Call-ID, the sequence number, the method and the branch before it is acted on |

## The layers, and what each was written against

### `g711.rs` -- ITU-T G.711

The encoders and decoders for both laws, as the segment reconstruction that
G.711 5.1 and 5.2's tables amount to, plus the float conversions the modem
side needs. `Law` also carries RFC 3551 Table 4's payload type numbers, 0 and
8, because the number on the wire and the law are the same fact and keeping
them in one place stops the two disagreeing mid-call.

The decision worth arguing about is the one stated in the table above: only
G.711 is ever offered. That is not a simplification to be relaxed later. It is
the reason the crate exists.

The tests check the round trip exhaustively in both directions rather than
against a table copied from somewhere, because the property that matters here
is not "the tables are right" but "a codeword that arrives is the codeword that
was sent". There is exactly one exception, and it is the law's rather than
ours: mu-law has two codewords for zero, 0xFF and 0x7F, and they decode to the
same sample. The encoder chooses 0xFF, so 0x7F is the single codeword in the
alphabet that a round trip changes. That is worth knowing before reading it as
a fault on a capture -- and it is one more reason to have nothing in the path
that decodes and re-encodes at all. A-law has its own oddity in the other
direction: its quietest codeword decodes to 8 rather than 0, so a silent A-law
line carries a very small square wave.

The second test is the more interesting one. For every sample an `i16` can
hold, it checks that the level the encoder chose is the closest the law has or
the one next to it, searched over the decoder rather than compared against a
table, so the two halves cannot agree on being wrong together. Not "the
closest", because it is not: inside a segment the encoder truncates where a
search rounds. Skipping a level would be an error; picking the neighbour is the
law's own quantisation.

### `md5.rs` -- RFC 1321, and `rand.rs`

MD5 because RFC 3261 22.4 defines the credentials in terms of it, and a
registrar that demands MD5 is answered in MD5 or it is not answered. Its
weakness as a hash is real and beside the point: the whole conversation travels
over plain UDP either way, and the password is never in it. Sixty lines,
checked against RFC 1321 A.5's vectors.

`rand.rs` produces branch parameters, tags, Call-IDs and client nonces from a
counter, the clock and the process id, mixed. SIP asks for randomness in
several places and means *uniqueness* by it every time -- a branch that matches
a retransmission to its own transaction, a tag that tells two calls between the
same pair of addresses apart. None of it is a secret, and pretending otherwise
by reaching for a cryptographic source would be decoration.

### `uri.rs` -- RFC 3261 19.1 and 20.10

A bare URI is what a request line carries; an address (20.10's name-addr) is a
URI in angle brackets with a display name in front and parameters after, and it
is what From, To and Contact carry. The brackets decide where the URI stops:
without them a semicolon starts a *header* parameter, with them a *URI*
parameter, so a registrar reading `sip:me@host;transport=udp` the wrong way
round registers a different address than it was sent. Everything this crate
builds uses the brackets; everything it parses copes with their absence.

### `message.rs` -- RFC 3261 7, 19 and 20

Start line, headers, blank line, body. The work is in 7.3.1: header names are
case-insensitive, most have a one-letter compact form meaning exactly the same
thing, a header may be folded across lines, and several may be combined onto
one line or repeated on several with equal meaning -- and a far end is free to
choose differently from one message to the next.

The decision: headers are kept as they arrived, in order, with their original
names, and looked up through a comparison that knows the compact forms.
Nothing is canonicalised on the way in. That costs a linear scan per lookup on
a list that is never longer than a couple of dozen entries, and it buys the
thing that actually matters -- a Via or a Record-Route echoed back exactly as
it was received, because a proxy in the path recognises its own Via by the text
of it.

### `auth.rs` -- RFC 3261 22, RFC 7616

The digest exchange: a 401 from a registrar or a 407 from a proxy carries a
challenge, and the same request goes again with a response computed over the
challenge, the method and the request URI. The two flavours differ only in
which headers carry them, and putting the answer in the wrong one is a fault
far ends do notice, so the distinction lives in the `Challenge` rather than in
the caller's memory. RFC 2069's no-qop shape is supported as well as qop=auth,
because a surprising number of registrars still send it, and MD5-sess because
it costs one more hash. SHA-256 (RFC 8760) is refused honestly by
`Challenge::supported` rather than answered wrongly.

The decision worth arguing about is in the parser, and it is the thing that
actually goes wrong in the field: the parameter list is comma-separated, and a
nonce is an opaque server blob that is quite entitled to contain a comma.
Splitting on commas without regard to quotes gives a truncated nonce, a
response computed over the wrong string, and the symptom everybody has met --
registration works with one provider and not another, for no reason visible in
the log. The splitter tracks quotes and escapes, and there is a test for
exactly that nonce. There is a second, duller rule the same code enforces: RFC
7616 3.4 makes username, realm, nonce, uri, response, cnonce and opaque quoted
strings and algorithm, qop and nc bare tokens, and registrars are strict in
both directions -- quoting `qop` gets a 400 from some of them and unquoting a
nonce gets a permanent 401 from others.

### `account.rs` -- the accounts file

`[section]` per account, `name = value` under it, `#` for a comment, at
`%APPDATA%\BinModem\sip.txt`. Four lines are a working account; everything else
has a default that suits an ordinary trunk. `Account::dial_uri` takes whatever
followed `ATD` -- spaces, brackets, dashes, the T/P/W/comma dial modifiers that
meant something on a copper line -- and produces a URI, passing a full
`sip:` URI through untouched so an extension with no number can be reached.

Two decisions. First, a misspelt key is refused by name and line number rather
than ignored. The key most likely to be misspelt is `password`, and a password
line that silently does nothing shows up only as a trunk that will not
register, with nothing in the log to say why. A refusal that names the line
costs a minute; the other thing costs an evening.

Second, and plainly: **the file holds the password in clear text.** That is not
a lapse to be tidied up later. SIP digest authentication (RFC 7616 3.4.2) takes
the password itself as an input to the hash, so there is nothing else that
could be stored -- anything reversible enough to compute a response with is a
password in a costume. The file is as private as the user's profile directory
and no more private than that, and the template that gets written says so in
those words.

`transport = udp | tcp` chooses which of the two `transport.rs` offers this
account uses. It defaults to udp and it is the only setting in the file that
changes what SIP itself does rather than what it says.

**The program now writes this file as well as reading it**, because the
credentials window writes what somebody typed into it, through
`Account::save_all` and `save_default`. That changed two things.

It is why quoting had to exist. The comment rule -- a `#` starts one at the
beginning of a line or after whitespace -- left a password of `a b # c`
unrepresentable: written down plainly it is a comment, so the value was
silently truncated, which is the exact class of failure the whole file is
arranged to avoid. So a value may be written in quotes, `""` inside them is one
quote character, and nothing starts a comment inside them. That is the only
escape there is, which is deliberate: enough to write any password down, and
little enough that somebody editing the file by hand does not have to learn a
language first. `quote_if_needed` quotes only what has to be quoted, so a file
the program wrote still looks like one a person wrote. This was found by the
test that writes an account and reads it back, which is what that test is for:
a round trip finds what neither direction looks wrong on its own.

And it is why `save_all` parses the text it is about to write, before writing
it, and refuses if what comes back is not what went in. That is not ceremony.
This is the one file in the program whose contents are a password, and a window
that wrote something the parser then refused would leave somebody with an
account that had quietly stopped existing -- and no reason to look in the file,
because they had just typed it into a window. `to_text` also writes only what
differs from the default, so an account typed into a window comes out as five
lines rather than fifteen, and the defaults stay in one place instead of being
copied into every file and frozen there.

### `sdp.rs` -- RFC 4566 and RFC 3264

Session descriptions, and the offer/answer model, for a call that will only
ever carry G.711. Most of the module is about refusing, and `read_answer`
returns a sentence rather than a code because the thing above it puts that
sentence in front of a person wondering why the call dropped: "the far end
answered with G729 (payload type 18)" answers that question in a way an error
enum does not.

Parsing is deliberately tolerant. Real far ends -- Asterisk, Kamailio, a
wholesale trunk -- write SDP that is legal but not uniform: bare line feeds
instead of CRLF, `c=` at session level only or repeated per media, no `a=rtpmap`
at all for the static payload types RFC 3551 already fixed, and vendor
attributes nobody outside the vendor has read. None of that is a reason to fail
a call, so unknown lines are kept or skipped and only three things are refused:
a body that is not SDP, a version other than 0, and an `m=` port that is not a
number.

The decisions worth arguing about:

- **The law is chosen by walking the far end's list, not ours.** The far end's
  first choice is nearly always the law its own trunk carries natively, and
  picking the other one puts a transcode in the path. That is exactly what
  turned a clean V.90 downstream into 37 dB of noise on the Crazytel trunk.
  Between two laws that are both lossless codeword paths, matching the far end
  is worth more than our own preference.
- **An `a=rtpmap` that contradicts the static table wins.** RFC 3551 Table 4 is
  a default; a far end that wrote `a=rtpmap:8 PCMU/8000` wrote it on purpose.
  Believing the table there decodes every sample of the call through the wrong
  law, which sounds like a badly distorted line and trains no modem at all.
- **`a=sendonly` is flipped.** It is a statement about the sender, so for us it
  means receive-only. A rig that copies it transmits into an ear that was never
  listening, which looks exactly like a far end that has gone deaf.
- **`c=IN IP4 0.0.0.0` outranks the direction attributes.** RFC 3264 8.4's old
  way of signalling hold, still sent. Taken at face value it means a call spent
  transmitting into the void while counting the returning silence as a far end
  that died.
- **ptime is clamped to 10-40 ms.** Every packet is a packet's worth of added
  delay in each direction before its contents can be looked at, on a path that
  already carries about 750 ms one way; V.32's echo canceller and V.34's
  half-duplex phases both have opinions about that. Below 10 ms the per-packet
  overhead costs more than the delay saved.
- **telephone-event is mirrored when offered and never generated.** RFC 4733
  replaces the tones with events and has the far end regenerate them at its own
  level, duration and gap, so the digits that reach the switch are not the
  digits that left here and a dial string timed to get past an IVR stops being
  timed at all. It is mirrored because some trunks refuse a call outright
  without it, and because an event we did not agree to still arrives -- agreeing
  at least tells us which payload type to throw away.

`wants_t38` recognises an `m=image`/`udptl` stream so that a fax far end is
declined clearly rather than answered with something that leaves both ends
waiting. Port 0 does not count: that is a fax attempt ending, not starting.

### `rtp.rs` -- RFC 3550 5.1

The fixed header, parsed and built, and nothing else: no reordering, no
concealment, no timing. Keeping that apart from `jitter.rs` is what makes
either testable.

Padding, a contributing source list and a header extension are all *parsed*
although none is ever *generated*, because what a far end sends is not this
end's choice and a payload read at the wrong offset is not audio. The parser
refuses a padding count of zero (5.1 makes the counting octet part of the
padding, so zero is a lie) and any length field that claims more than arrived.

No RTCP is sent. A trunk carrying one modem call for one user agent has nothing
to do with reports except count them. What is here instead is `is_rtcp`, by RFC
5761 4's test -- RTCP's packet type fills the whole second octet and is 192 to
223, which RFC 3551 6 keeps unassigned as RTP payload types for exactly this
purpose. A far end that does not honour the port pair sends its reports to the
RTP port, and a report decoded as G.711 is a burst of loud noise straight into
the modem. Recognising them is not politeness.

### `jitter.rs` -- RFC 3550 A.1, inverted

This is the module that is the opposite of what a telephone wants, and it is
the point of the crate.

Every softphone's jitter buffer is adaptive, and that is correct for speech: a
voice can be stretched a few milliseconds during a pause and nobody hears it,
so the buffer watches arrival times, grows when the network wobbles and shrinks
back by swallowing a little audio when it settles. None of that reasoning
survives contact with a modem. A modem is a clock recovery loop with a decision
device hung off it; audio inserted into its input is not a small delay, it is a
phase step, and the receiver has to fall out of lock and find the symbol
boundaries again. That is the measured 1.3 s above.

So this buffer is fixed. It never resamples, never stretches, never quietly
drops a packet to catch up, and never adapts its depth -- adapting means
changing the length of the audio, and changing the length of the audio is the
fault. The defaults in `media.rs` are 2 packets of cushion (40 ms, enough to
put back in order the pair-swaps an ordinary path produces, and nothing beside
the delay already in the line) and 50 to hold before shedding. A telephone
would use ten times the cushion and stretch audio to manage it. A capacity that
is not at least one packet above the target is raised rather than obeyed,
because a buffer that sheds every packet the moment it lands on top of the
cushion hands over nothing at all: a line that is up and silent, which is
harder to diagnose than a loud mistake.

**`target` is a depth, not a starting gun.** This is written out because it was
wrong here, it is an easy mistake to make twice, and getting it wrong broke the
one measurement the crate exists to produce. `drain` used to prime once -- wait
for `target` packets, set a flag -- and then empty the map on every later call.
The caller drains about ten times per packet interval, so from the second drain
onwards the steady depth was one packet or none and the cushion was simply not
there. An ordinary pair-swap, the exact case `target` is sized for, then came
out as a concealed 20 ms hole *and* a `late` packet: a fault invented by the
buffer and charged to the network. So the rule is now a depth, tested on every
drain -- hand over while more than `target` packets are waiting, which is what
leaves `target` of them in hand afterwards, at packet one and at packet ten
thousand alike.

Two consequences follow from that and both are meant. The line starts `target`
packets late, which is the delay being bought. And when the far end stops
sending, the last `target` packets stay here rather than coming out: the buffer
cannot tell the end of a call from a pause, and inventing an end would mean
handing over a gap that later turns out not to be one. Those are counted as
`abandoned` when the buffer is restarted, rather than going quietly -- audio
discarded in silence is the same lie as audio inserted in silence.

Concealment is silence, one packet's worth, in the caller's quiet codeword
(0xFF in mu-law, 0xD5 in A-law -- a buffer of zero octets is a loud tone in
both). Not the previous packet again, not an interpolation, not a fade: a
repeat of audio the modem has already had is precisely the 20 ms insert that
broke V.34 here, and it is worse than a gap because it is plausible. A gap is a
dropout, which a receiver knows how to recognise and which a real line
produces: the energy goes away, the equaliser and timing loop coast, error
control asks for the frame again. And every octet of it is counted. A buffer
that hides its own interventions turns every later measurement into a lie,
which is exactly what happened before.

Five smaller decisions:

- A missing packet is concealed only once `target` packets have piled up behind
  it. Until then it may still be in flight, and the delay of waiting is the
  delay this buffer exists to spend. That is the same rule as the cushion, read
  from the other side: the cushion is what proves a gap is a gap.
- A concealment is one packet long, and a packet is the length the *stream* has
  settled on rather than the length of the last thing that arrived. Three
  packets in a row at a new length take it up, because a re-INVITE really can
  change the packet time mid-call; one odd packet does not. It used to be
  whatever arrived last, so a single four-octet RFC 4733 event packet left every
  later concealment four octets instead of a hundred and sixty -- the audio
  short by the difference and `concealed` under-reporting it by a factor of
  forty, in the direction that says the network was fine.
- The sequence number is extended to 64 bits by counting wraps, along the lines
  of A.1's `update_seq`. Sixteen bits at fifty packets a second is twenty-two
  minutes, and a download runs through that. Getting it wrong does not cost a
  packet, it costs the rest of the call, because every packet after the wrap
  looks ancient. There is a test for the awkward case: 65535 arriving after 0
  is numerically far ahead and actually just behind.
- A jump forward of more than A.1's `MAX_DROPOUT` is not a gap in this stream
  but a far end that has started another one at a fresh random sequence number,
  and it is counted in `resynced`. Filling it with silence to keep the timeline
  would mean building tens of minutes of it in a single `drain`, in one
  allocation, while the modem loop waits -- for a hole that was never in the
  line. The timeline steps to the new numbering instead and says so.
- Unlike A.1 there is no probation period and no source validation. A.1 guards
  against a stream from somewhere else being adopted; this buffer is fed by a
  reader that has already thrown away RTCP, anything that is not RTP and
  anything that is not the payload type we negotiated, and a modem call cannot
  afford to discard the first two packets of a stream while it makes up its
  mind.

On overflow the *oldest* packets go, without concealment. Audio the modem never
got is worth nothing to it, and the thing actually being shed is the delay
those packets represent; concealing would emit the same number of octets and
shed no delay at all. What went is counted twice over, as `overflowed` packets
and `overflowed_octets`, so that an overflow can be lined up against a capture
the way a concealment can -- a count of packets alone cannot be turned into a
length of line.

### `media.rs` -- the audio path

Two threads and two queues.

A **reader** thread does nothing but pull datagrams off the socket and put
their payloads into the jitter buffer, because a thread that is doing anything
else is a thread that is not reading the socket, and the operating system's
receive buffer is the only thing between a scheduling hiccup and a lost packet.

The one judgement it makes is what to let through. RTCP is recognised and
dropped, anything that will not parse as RTP is dropped and counted in
`ignored`, and **anything whose payload type is not the one negotiated is
dropped and counted in `off_codec`**. That last is here rather than in
`jitter.rs` because this is the only layer that knows what was agreed; the
buffer is handed a stream and cannot tell an agreed codec from an unexpected
one. It matters more than it sounds. An RFC 4733 telephone-event packet is four
octets, our answer mirrors a far end's offer of them so an incoming call has
told the far end it may send them, and one dialled digit is ten to twenty of
them. Comfort noise is one octet. Every one of those used to be decoded as
G.711 and handed to the modem, which is a 20 ms deletion of the audio timeline
per packet, uncounted -- precisely the phase step this crate was written to take
out of the path. The sequence number such a packet used is now left as a gap,
which the buffer conceals to the right length, so the timeline keeps its length
and `off_codec` and `concealed` climb together. That pair is how to read it: a
few of each is somebody dialling, and `off_codec` climbing steadily with no
audio behind it is a transcoder in the path and a call that is over whatever we
do.

A **pacer** thread sends one packet every packet time, whatever else is
happening. This is the part worth arguing about. The alternative -- send a
packet whenever one arrives, so the far end's clock is ours -- is tempting, and
is effectively what a gateway does. It is rejected because it makes our
transmission stop if the far end's ever does, and a far end applying silence
suppression does exactly that. So: our own 20 ms clock, and when the modem has
not produced a packet's worth we send silence and count an underrun. That is
the same bargain `line::Duplex` makes with the sound card, for the same reason
-- a far end hears a dropout, which is what error control is for, where a stall
is a line that has gone away. When the machine has been busy elsewhere and the
pacer is badly behind, it restarts from now rather than sending a burst to
catch up: a burst arrives at the far end as jitter, which is the thing this
whole crate exists to avoid inflicting.

**Symmetric RTP.** The far end is followed to whatever address its audio
actually came from, and the first time that differs from what it advertised,
`latched` is incremented. The address in the far end's SDP is the address it
believes it has, which behind a router is not where its packets originate, and
sending to the SDP address is the classic one-way-audio call. This is what
makes the crate work from behind a home router at all, and it is why no ICE or
STUN is needed.

It follows once. The first packet of a call says where the stream is, and
after that another address is followed only if the packet from it is the same
stream moving there: the same SSRC, carrying on within two seconds of where
the stream had got to (RFC 3550 8.2 and A.1). Anything else from elsewhere is
dropped and counted in `strangers`. Following every new source had let one
datagram from anywhere take the call's audio over in both directions.

**Connecting twice to the same call is nothing at all.** `Media::connect` used
to restart the jitter buffer and empty the outgoing queue every time it was
called -- up to fifty packets in and twenty-five out, thrown away without a
word. The path that reaches it is not rare: a 200 OK that arrives twice, or any
re-INVITE, sends `line::Line::poll` back through it in the middle of a call
that nothing was wrong with, and on this rig that is the twenty-millisecond
discontinuity that costs a V.34 receiver its training -- arriving at the one
moment every counter says the line is clean. So the address last dialled is
kept apart from the address the audio is actually going to (symmetric RTP moves
the second one), and a connect naming the same place, the same payload type and
the same packet time returns without touching anything. A connect that really
does change the call still throws both away, but says how much went with them:
`abandoned` for the buffer and `flushed_out` for the queue. The difference
between a discontinuity and an unexplained one is the number.

**The outgoing queue starts with one packet of silence in it.** The modem only
produces samples when a packet arrives, so without priming the queue starts
empty and the pacer's phase against the far end's arrivals is whatever it
happens to be; any wobble inbound leaves the queue empty at the moment the
pacer looks, and 20 ms of silence goes out counted as an `underrun` that
nothing here did wrong. `underruns` would then read non-zero on a healthy call
and mean the opposite of what it says. One packet of cushion removes the whole
class. It costs 20 ms of one-way delay, once, on a path that already carries
about 750 ms, and the octets are the same silence an underrun would have sent
-- the difference is that these are ours, sent on purpose.

What is deliberately absent: no gain control, no voice activity detection, no
comfort noise, no packet loss concealment beyond silence, no adaptive anything.
Every one of those improves a telephone call and damages this one.

Two more details. The outgoing queue is capped at 25 packets' worth; anything
waiting there is delay added to a path that already has about 750 ms of it, and
a queue that has grown past that is not going to drain. And the socket stays
bound across calls: a port that moves between calls is a port some far end is
still sending the last call to. What is left in the queue when a call ends is
counted into `flushed_out` too -- it is ordinarily the last few tens of
milliseconds of whatever the modem was saying, and the only way to tell that
from a call cut off mid-sentence is the number.

### `transport.rs` -- RFC 3261 18, and 7.5 for the framing

The link to the next hop: the file that holds either the datagram socket or the
connection, and every consequence of the difference bar the three the agent
cannot delegate. It is declared privately from `ua.rs` rather than published
from `lib.rs`, because which socket the octets leave by is not part of what
this crate offers and nothing outside `ua` has any use for it.

Everything above it is dialogs, transactions and timers, and none of that
changes with the transport. Three things do, which is the whole reason the
module exists:

- **Framing (7.5).** Over UDP a datagram is exactly one message and there is
  nothing to decide. Over a stream there are no edges: a message is found by
  reading to the end of the headers and then taking exactly Content-Length
  octets of body. One read may hold two messages, or a third of one.
- **Whether the protocol retransmits (17.1.1.2, 17.1.2.2).** `Link::reliable`
  is what the timers ask, and the answer is the whole of the difference.
- **What the Via and the Contact say (18.1.1, 19.1.1)**, which is
  `Link::transport`'s to answer and the agent's to write.

**The framing is a pure function.** `frame(&[u8]) -> Frame` reads a buffer and
says what is at the front of it: `Incomplete`, `Skip(n)`, `Whole(n)` or
`Broken`. It touches no socket and holds no state, and that is the decision
worth arguing about, because the alternative -- framing inside the read loop,
against a real stream -- is the ordinary way to write it. Framing is where a
stream transport is got wrong, and the faults are exactly the ones that only
happen when the line is busy: two messages in one read, a body arriving after
its headers, a message with no body at all followed immediately by another. A
function that needs a socket to be tested is a function those cases do not get
tested on, and they are the cases. As a pure function they are eight lines of
test each, and there is one for every one of them.

Three things follow from `frame` reading raw octets rather than a parsed
message, and all three are the point:

- **Content-Length is read out of the header octets.** It has to be: the
  message cannot be parsed until this has said where it ends. It costs a small
  scanner that knows 7.3.3's compact form `l` and 7.3.1's line folding, and it
  buys the property that a far end which puts something unparseable in a From
  costs one unreadable message rather than a connection that can no longer be
  framed at all. `Message::parse` accepts bare line feeds as well as CRLF, so
  the blank-line search here accepts both too: the two agreeing about where a
  body begins matters more than either being strict.
- **A message with no Content-Length ends the connection.** 7.5 makes the
  header mandatory over a stream, and it has to be -- the body runs to wherever
  the far end decided, so not only is that message unreadable, every message
  after it on the connection is too. 3261 has a server answer 400 and close;
  only the closing is done here, because the answer would have to be built from
  headers that have just proved untrustworthy and sent over a connection that
  is going anyway.
- **What is plainly not SIP ends it too, early.** Over UDP a port scan, a TLS
  ClientHello sent to the plain port or an HTTP request is one bad datagram.
  Over a stream it is a position nothing can be resynchronised from, so the
  start line is checked for a control octet and for 7.1 or 7.2's version string
  before anything waits on a Content-Length that is never coming. An
  incomplete start line is not rubbish, and is not treated as any.

Everything unbounded is bounded: the body at 128 KiB, the headers at 16 KiB,
the start line at 1 KiB. A far end declaring four gigabytes must not become
four gigabytes of ours. That also stands in for 18.3's five-second timer, which
is not implemented -- a size limit and a closed connection get to the same place
for the case that actually occurs.

**Reads are a read timeout, not a reader thread.** The agent is a set of timers
serviced by one thread, and that thread wakes every 20 ms because the UDP
socket's read timeout says so. The TCP stream carries the same read timeout, so
a read that finds nothing returns to the timers exactly as an empty datagram
socket does, and the agent keeps one pacing mechanism instead of two. The other
shape -- a reader thread feeding a queue, which is how `media.rs` is built --
would work, and was rejected for two reasons. The framing state and the octets
it is scanning would live behind a mutex on the far side of a queue from the
only code that understands them. And a queue that is never empty would have to
be paced by something other than the read itself, which is the one thing that
currently guarantees the timers get looked at. A thread buys concurrency that
one call at a time, on one connection, has no use for. When several messages
arrive together they are returned one to a call without waiting, so a busy
moment is not spread out at one message per 20 ms.

Smaller decisions, each because of a thing that happens: Nagle is off, because
it would hold the last part of a message back waiting for more to say and this
line carries about 750 ms each way. A connection is attempted when the link
opens, so the first request out carries a sent-by with a real port in it, and a
failure there is not fatal -- a trunk that is down at start-up is an ordinary
Tuesday and the link tries again at most once a second. The read loop
reconnects as well as the write path does, because a trunk sends an inbound
call over the connection it has, and an agent that only connected when it had
something to say would not have one to send it over. A half-written message
drops the connection rather than the framing: the far end has no way of telling
where the next message starts, and 7.5 gives it nothing to resynchronise on.
Anything half-read is thrown away with the connection it was arriving on. And
7.5's stray CRLFs before a start line are skipped, which also disposes of RFC
5626 3.5.1's double-CRLF keep-alive that several trunks send every half minute.

The media path is untouched by any of this. RTP is UDP whatever SIP travels
over, and that stays `media.rs`'s business.

### `ua.rs` -- RFC 3261, the subset a modem needs

That subset is smaller than the document but not as small as it first looks,
because what can be skipped and what cannot are not sorted by how interesting
they are.

What is implemented: REGISTER with digest authentication and refresh at 9/10 of
whatever expiry the registrar granted (10.2.4 -- the registrar decides, not us);
INVITE, ACK, CANCEL and BYE as a caller; incoming INVITE answered 100, 180 and
then 200 on `ATA`, 486 when the one line is busy and 603 on decline; re-INVITE
inside a dialog; OPTIONS answered, because several trunks send one every thirty
seconds and answering it is what keeps the registration usable; INFO and UPDATE
answered 200; anything else 405. 17.2.1's absorption of retransmitted requests
is there, keeping the response sent for five seconds -- the final one, and
until there is a final one the last provisional -- so a repeated request gets
the same answer rather than a new one. Over UDP on a slow path that is not an
edge case, it happens on most calls. Every final response now leaves through
one function, `send_final`, so that filing it cannot be forgotten: the two
where it mattered most -- the 200 that answers a call and the 200 that answers
a re-INVITE -- were the two that used to be built by hand and sent directly,
and so the two that were never filed at all.

Three pieces are there because without them a call does not work at all, and
none is optional in practice:

- **rport (RFC 3581).** Without it the far end sends its responses to the
  address written in the Via, which behind a router means nothing outside this
  machine. With it they come back to the port the request came from, which is
  the only one the router has a mapping for. The address the registrar reports
  back is also what the Contact is then built from.
- **The route set from Record-Route (12.1.2)**, reversed for a caller and kept
  in order for a callee, sent as Route headers on everything in the dialog, with
  the datagrams going to the first route's host rather than the target's. A
  trunk that record-routes and is ignored routes every in-dialog request into a
  hole. The other half of the same rule is 12.1.1: the Record-Route headers a
  caller's INVITE arrived with are copied into our own 2xx, in order, because
  that response is the only place the *caller* can get its route set from.
  Leaving them off sends its BYE straight at our Contact and into the same
  hole, and its half of the call stays up and goes on being billed.
- **Matching a response to a transaction.** A response is acted on only when
  the Call-ID, the sequence number, the CSeq method and the Via branch all
  agree with a request we actually sent (17.1.3). This looks like bookkeeping
  and is not: responses used to be dispatched on the CSeq method alone, and on
  a path that retransmits everything, a duplicate of a real response from a
  moment ago has every field right except the ones that say which request it
  belonged to. The method is the part that is easy to miss -- 9.1 gives a
  CANCEL the INVITE's branch and the INVITE's sequence number on purpose, so
  the method in the CSeq is the only thing telling those two transactions
  apart.

What is left out, deliberately: TLS, ICE, SRTP, a general transaction layer,
forking, and the subscribe/notify machinery. One transport at a time, one call
at a time, and the handful of transactions a call needs. A proper stack keeps a
table of transactions keyed by branch because it might have hundreds; this has
at most four, so they are four fields and the code servicing them reads in one
sitting.

Almost none of this file knows which transport it is on. `transport::Link`
holds either the datagram socket or the connection and its half-read buffer,
and offers the two things the agent wants -- put these octets on the wire, and
give me the next message that has arrived. Three places do know, and they are
the three RFC 3261 makes different: the framing, which is the next section's
business; the Via's sent-protocol and the Contact's `;transport=tcp`, which have
to name the transport the message went out over (18.1.1, 19.1.1); and the
retransmission timers. The Via matters more than it looks -- a proxy told UDP
by a request that arrived over a connection answers to the address in the
sent-by, which over TCP is a port nothing is listening on -- and so does the
Contact, because without it a trunk reads 19.1.2's default and sends a
re-INVITE or a BYE as a datagram to that same port. rport is asked for over TCP
as well, where 18.2.1 makes it redundant for routing, because the `received`
that comes back with it is still how we learn what address the world sees.

Three more rules that a slow path makes compulsory rather than academic:

- **A 2xx to an INVITE is retransmitted until it is acknowledged (13.3.1.4),
  and a retransmitted 2xx from the far end is re-ACKed and otherwise ignored
  (13.2.2.4).** A 2xx is the one response with no transaction underneath it --
  the INVITE server transaction ends the moment it goes out -- so getting it
  through is the user agent's own job. In the other direction, this path
  carries about 750 ms each way, so the far end always gets at least one
  retransmission out before our ACK can reach it: it fires on essentially every
  call. Every 2xx used to be handled identically, so the dialog was adopted
  again, the answer read again, and another `Answered` raised -- which the layer
  above turns into `Media::connect`.
- **Provisional responses are remembered as well as final ones.** 17.2.1 has a
  request that arrives again answered with what it was answered with before,
  and only final responses were being kept. A call ringing here has had a 100
  and a 180 sent for it and neither is final, so a retransmitted INVITE fell
  past the absorber into the dialog handling -- where being in a dialog was
  decided on the Call-ID alone -- and was answered 200 by a telephone nobody had
  picked up. Being in a dialog now needs the Call-ID *and both tags* and an
  established call (12.2.2), and a retransmission while ringing gets the 180
  again.
- **A CANCEL waits for a provisional response, and then retransmits.** 9.1 says
  a client must not send a CANCEL until the request it cancels has been
  provisionally answered, and `ATD` followed by `ATH` inside 750 ms is exactly
  the race that rule is there to prevent: the CANCEL reaches the proxy before
  any server transaction exists to match it, the proxy answers 481, our INVITE
  goes on being retransmitted, the trunk rings the number, somebody answers,
  and the 200 brings up a call the user hung up on and is billed for. So the
  wish is kept and spent the moment the far end says anything. Once sent it is
  an ordinary non-INVITE client transaction (17.1.2.2) and retransmits like
  one; it used to be sent once and forgotten, so one lost datagram was
  indistinguishable from never having hung up. And if the far end answers
  anyway -- which 9.1 says happens -- the dialog is real, so it is acknowledged
  and then ended with a BYE rather than dropped.

Timers are 17.1.1.2's, unchanged: T1 at 500 ms, doubling, capped at T2 (4 s)
for non-INVITE requests and uncapped for an INVITE, Timer B at 64*T1 = 32 s.
An INVITE is given 120 s rather than Timer B before it is called dead, because
a trunk ringing a real telephone can take that long and sends provisional
responses while it does. The temptation on a known-slow path is to start T1
higher; the reason not to is that these timers are what a far end's own
duplicate suppression is built around. On this rig, carrying about 750 ms one
way, the first retransmission of anything is normal and not a sign of trouble.

Over TCP the retransmissions do not run at all. 17.1.1.2's Timer A and
17.1.2.2's Timer E are turned off over a reliable transport, and so is
13.3.1.4's resending of a 2xx, which is the same rule seen from the other end:
something underneath has already tried again, and a request sent twice down a
connection is a duplicate the far end has to disentangle for no reason -- and
on a proxy that answers the second copy separately, a duplicate that earns its
own 481. What still runs is every deadline: Timer B, Timer F, the ring limit
and the 2xx's give-up. A far end that never answers is not something a
transport can fix.

That leaves one gap, and it is the one place above `transport.rs` that has to
know what it is on. A connection that breaks takes the delivery guarantee with
it: a request written to the socket that died may have arrived or may not,
nothing underneath will try again, and the transaction would otherwise sit out
the whole of Timer B waiting for an answer to something nobody was ever sent.
So when the link reports that it has remade the connection, everything still in
flight -- the registration, the INVITE, the CANCEL, the BYE, and the 2xx that is
waiting for an ACK -- goes out once more on the new one. Once, not on a timer,
for precisely the reason the timer was turned off. The registration is also
refreshed immediately rather than at nine tenths of the expiry, for a different
reason: nothing about it is cleared, so the line does not report itself
unregistered over a hiccup the trunk may not have noticed, but the binding the
registrar is holding names a connection that has gone, and 18.2.1 is how an
inbound call would have come down it.

Authentication has one rule beyond `auth.rs`'s: **a second challenge marked
`stale=true` with a new nonce is answered again.** One answer to one challenge
is authentication and two in a row normally means the password or the realm is
wrong, which is why the second used to be refused outright -- but `stale` was
parsed and read nowhere, and it is the case that actually happens. An
Asterisk-family registrar expires its nonces on a timer and re-challenges with
a fresh one, which RFC 7616 3.3 makes a cue to try again and not a statement
about the password at all. Refusing it dropped a working account into a
thirty-second backoff, over and over. The nonce has to have changed, or "stale"
would be a licence to loop on the same one, and three attempts is the bound
however honest the registrar looks. Which of the two it was is said in the
transcript, because one is a setting to change and the other is a provider to
complain to.

Two smaller things that only show up as a call that never happens. **An IPv4
address is preferred** when a trunk's name resolves to both, because the socket
is bound to `0.0.0.0` and can speak nothing else; a trunk whose name answers
AAAA first -- and plenty do -- failed every `send_to` with `AddrNotAvailable`.
And **a send that fails is said once per transaction**: every send used to be
`let _ = ...`, so the whole of that failure was invisible and the only thing
anybody saw was "dialling" for the full two minutes of the ring limit before
being told nobody answered. Retransmissions stay quiet -- one line a
transaction is a diagnosis, one line every half second is a log nobody reads.

One thread owns the socket and everything derived from it, and the layers above
talk to it through a queue of commands in and a queue of events out -- the same
arrangement the audio line uses. A SIP agent is a set of timers, and the thread
that owns the socket is the only place a timer can be serviced promptly without
a great deal of ceremony about who may touch what.

Failure codes are turned into sentences (`describe`) because the number alone
tells a person nothing: 403 from a trunk usually means the number, the caller
ID or the account is not allowed to dial it, and that is worth saying.

### `line.rs` -- the same two methods a sound card has

`Agent` knows about dialogs and `Media` knows about packets; neither knows
about the other, and something has to. `Line` is that something: it opens the
media port first so the offer can name it, starts the agent, and when a call is
answered points the media at whatever the two ends agreed to.

The shape is deliberately `line::Duplex`'s. `receive(&self, into: &mut Vec<f32>)`
fills a buffer at the modem's rate; `transmit(&self, samples: &[f32])` takes
one. The loop driving the modem does not have to know which kind of line it has
got. The one genuine difference -- a sound card is either open or not, where a
call is idle, ringing, up or over -- is carried in `Progress` rather than in the
sample path.

**A consequence falls out for free: the modem is clocked by the call.** Before
the far end answers there are no packets, so `receive` hands over nothing, so
the modem is never stepped and does not start its handshake into a call that
has not connected. That was not arranged; it is what the arrangement does. It is
also the right behaviour -- a modem that starts transmitting at ringback is a
modem whose first second of V.8 was spent talking to a switch. (The sound-card
line has the same hazard from the other direction and pays for it explicitly:
`duplex.rs` throws away a quarter of a second of settling input because a V.32
modem once ran its entire round-trip measurement inside the first fifth of a
second against a far end that had not started.)

`poll` must be called regularly, once round the line loop, because that is
where an answered call is connected to its audio. The agent thread deliberately
does not touch the media itself: one thread owns the socket and its timers, and
a call being answered is the one moment the two sides have to meet.

Early media (183 with a description) is reported and not connected to. Ringback
and announcements are not what the modem is here for, and a far end that sends
early media and then answers with a different description would leave us
sending to the wrong place.

**The far end's `c=` line is resolved rather than parsed.** `c=IN IP4
sbc.example.net` is legal SDP -- 4566 5.7 allows a host name -- and some
Asterisk configurations emit one. This used to be
`format!("{address}:{port}").parse::<SocketAddr>()`, which fails on anything
that is not a literal, and the call was then hung up with "the call came up but
the audio could not", which reads as a far-end fault and is not one. So a name
is looked up. It is a blocking lookup on the thread that polls the line, which
is worth saying out loud, but it happens once when a call is answered rather
than per packet. An IPv4 answer is preferred, for the same reason the SIP side
prefers one, and an answer that leaves only IPv6 is refused here in a sentence
that names *this* end as the one that cannot do it -- rather than being accepted
and then sent to from a socket that cannot reach it, which would run the whole
call with `packets_sent` stuck at zero and no reason given. An IPv6 literal is
parsed as an address and not as "host:port", because a `c=` line carries one
with no brackets round it and `SocketAddr`'s parser wants brackets.

**The direction in the description decides whether the modem may transmit.**
`Negotiated::direction` is ours -- `sdp.rs` has already flipped the far end's
words -- so receive-only means the far end will send and will not listen, and
inactive means neither end will. Either way the transmitter is shut and the
media path is left exactly as it is. That last part is the fix: hold by RFC
3264 8.4's old convention is `c=IN IP4 0.0.0.0`, which parses as a perfectly
good address, so it used to be taken for a fresh description -- the pacer
pointed at 0.0.0.0 and the jitter buffer and the outgoing queue both restarted,
which costs the modem its training over a hold it could have sat through. The
pacer goes on sending its packet every packet time, which during a hold is
silence, so that the address binding through whatever router is in the way
stays open and audio can arrive the moment the hold is taken off. A
receive-only answer is said plainly in the transcript as well, so that the
forty seconds of failed training that follow a one-way call are not a mystery.
The transmitter is opened again when the call ends, or a hold nobody took off
would leave the next call silent with nothing in the transcript to say why.

At the end of every call, `closing_report` says what the call cost in packets.
That is the section below, said automatically so it does not have to be worked
out afterwards from a capture. Everything that is not the network's fault --
`off_codec`, `flushed_out`, `abandoned`, `resynced` -- is said separately and
only when it happened, because the whole use of the loss numbers is that they
are the network's, and a call blamed on a lost packet that was really thrown
away in here would be debugged in the wrong place for a week.

For the AT layer to reach this, `modem::Modem` grew `take_dial_request` and
`take_answer_request`: a telephone line does not need the dialled string --
the modem goes off hook, plays the digits and the switch does the rest -- but a
line made of packets has to place a call before there is a line at all. Every
other kind of line drops those on the floor. It also grew `carrier_lost`, for
the other end of the same difference: a modem on copper never had to be told
the carrier had gone, because its own detector noticed, where a line made of
packets ends in a BYE somewhere above the samples. `crates/gui/src/live.rs`
calls it when a call goes away under a modem that is still waiting -- refused,
cancelled, hung up at the far end, or the whole line closed underneath it --
and that is what gets `NO CARRIER` back to the terminal instead of `OK`, or
instead of a silence that lasts until somebody types `+++`.

## The windows that drive it

Outside this crate, in `crates/gui/src/app.rs`, there are now two windows.
They are worth a paragraph here because they are what decides which of this
crate's settings anybody ever touches.

**The dialler** is shaped like the softphone it replaces: a number at the top,
a telephone keypad under it with the letters printed on the keys, a button that
says Call and becomes Hang up, and at the bottom whether the trunk believes who
we are -- Online, Offline, or "No registration" for an account that does not
register, which is not the same as being offline and must not be shown as it.
That last line is there because whether the registration is good is the one
fact about an account that has to be visible *before* a call rather than worked
out from a failed one afterwards. The keys have two departures from a
telephone, both because a packet network is not a switch: there is no hook
flash, since a quarter-second on-hook tells a switch something and tells a
trunk nothing, so R redials instead -- a number that did not answer is the one
most likely to be wanted again; and every button types what a person would have
typed. `ATD` goes through the terminal's own path rather than into `sip::Line`
directly, so the modem sees the same command whichever way the call was placed,
the terminal shows it going out, and there is one way to place a call instead
of two that can drift apart.

**The credentials window** is a form leading with the SIP server, the username,
the password and the UDP/TCP choice, which is the order a provider's own portal
gives them in. It is a separate window rather than a fold inside the dialler
because the dialler is a keypad and is the width of a keypad, where this has to
hold a registrar's name; one of the two would have had to be the wrong shape.
It writes through `Account::save_all`, and it is honest about the cost: a save
rewrites the whole file, so comments somebody added there by hand do not
survive it. The file remains a text file in the user's profile directory,
readable and hand-editable, and still the thing that is loaded. The window only
saves having to go and find it.

## What is not done yet

| Not done | Why, and what it would cost |
|---|---|
| T.38 | The stated long-term goal. It needs a different transport (UDPTL, not RTP) and its own packet format, which is a real piece of work rather than a flag. Today an `m=image`/`udptl` offer or re-INVITE is recognised and declined with 488, which leaves the call on G.711 where the V.17, V.29 and V.27 ter modulations in `crates/fax` already work. See the sketch below |
| TLS and SRTP | The call is plaintext at the far end regardless, so encrypting the first hop would hide nothing worth hiding while making every capture unreadable. If a provider ever requires TLS, this becomes necessary rather than desirable. `transport.rs` is where it would go: a third arm of `Link`, with the framing above it unchanged |
| 18.1.1's switch to TCP at 1300 octets | Required, and not done: a request within 200 octets of the path MTU is supposed to go over TCP whatever the account says. What it protects against is a fragmented datagram, which more routers drop than one would like, and an INVITE with a long SDP is the message that gets there. Ours is short -- two payload types and a ptime -- so nothing here comes near it today. The cure is the same either way: set `transport = tcp` on the account |
| A TCP listener | There is no `TcpListener`, so a far end that opens a connection to the address in our Contact is not heard. In practice a trunk sends an inbound call down the connection we already have to it, which 18.2.1 is about and which does work; what is missing is the case of a far end that wants a connection of its own. Related: `local_port` is ignored over TCP, because std cannot bind a source port for an outgoing connection, and with no listener there is nothing for a fixed port to be useful to |
| RFC 3263, and 18.2.1's fallback connection | The transport comes off the account rather than out of an SRV or NAPTR lookup, and a response that names a different address in its Via is not connected to -- it goes back down the connection its request arrived on, which is what every trunk does. Both would matter for a provider that expects a client to discover it |
| ICE and STUN | Not needed for the case that exists: symmetric RTP plus rport gets through an ordinary home router, and the `latched` counter says when that has happened. A far end behind a symmetric NAT of its own would need them |
| More than one call at a time | The agent has one dialog, one INVITE slot, one CANCEL slot, one BYE slot, and answers a second incoming INVITE with 486. Nothing above it wants two calls; lifting this means the transaction table a real stack has |
| RFC 4733 DTMF generation | Offered and mirrored, never sent. A modem wants its DTMF as audio, because events are regenerated at the far end's choice of level, duration and gap. Only needed if a trunk is found that will not pass in-band digits at all |
| Session timers (RFC 4028) | Not implemented and not requested. A trunk that insists on them will hang a call up periodically; if that is seen, it is a Session-Expires header and a re-INVITE on a timer |

## T.38, sketched

This is a sketch of the shape of the work, not a plan. Nothing here has been
designed against the Recommendation in detail yet.

T.38 carries T.30 -- the fax protocol itself -- over packets rather than over a
modulated carrier. The pieces:

- **UDPTL as the transport.** Not RTP. It is a different packet format on a
  different `m=` line (`m=image <port> udptl t38`), which means the media side
  grows a second kind of socket handling rather than a second payload type.
  RTP-over-T.38 exists in the Recommendation and almost nothing deploys it.
- **IFP packets** carrying T.30 indicators (the preambles and tone states that
  a modulated call would signal by transmitting a carrier) and HDLC data (the
  T.30 frames themselves, with their own flags, addressing and FCS handled as
  data rather than as bits off a demodulator).
- **The error correction scheme.** UDPTL has no retransmission, so T.38 carries
  redundancy: each packet repeats some number of previous IFP packets, or
  carries FEC over them. Choosing and honouring that is most of the work of a
  receiver, and the choice is negotiated in SDP
  (`a=T38FaxUdpEC:t38UDPRedundancy`).
- **The re-INVITE dance.** A live audio call detects a fax tone and one end
  sends a re-INVITE offering `m=image`/`udptl` with the audio stream withdrawn;
  the answer accepts, and the media path switches from RTP audio to T.38 packets
  mid-call. It can switch back the same way. `ua.rs` already recognises both
  arrival points -- an initial INVITE that asks for T.38, which is answered 488
  before the telephone is ever put on the hook, and a re-INVITE that does, which
  is answered 488 with the call left exactly as it was -- and those two places
  are where the switch would be made. Both refusals name T.38 in the reason
  phrase, so a person reading a capture is not left wondering what was not
  acceptable. `sdp.rs::answer` has a comment at the exact point where the
  `m=image` answer would be built, listing the attributes it would need, and
  `sdp::wants_t38` is what both places ask -- it ignores a stream offered at
  port 0, because that is a fax attempt ending rather than starting. The
  re-INVITE path has an integration test in `tests/call.rs` whose real subject
  is not the 488 but what must *not* happen around it: the call is not torn
  down, the law does not change, and audio keeps flowing after a renegotiation
  that was refused.

The reason this is a bounded job rather than an open one: BinModem already has
the T.30 layer (`crates/fax`: `t30.rs`, `frames.rs`, `ecm.rs`, the T.4/MMR
coding) and the V.17, V.29 and V.27 ter modulations. T.38 does not need any of
the fax protocol to be rewritten. It needs T.30's frames and indicators routed
to a packet transport instead of to a modulator, and the redundancy scheme
built underneath. That is a real piece of work with a known shape, which is
different from the rest of this crate only in that none of it has been started.

## What to measure on the first live call

No live call has been placed with this crate. When one is, these are the
counters that exist and what each one means. All of them come out of
`Line::stats()`, which is `media::Stats` with `jitter::Counters` inside it, and
the handful worth showing while a call is running -- packets each way, `lost`,
`concealed`, `underruns` -- also come out of `Line::progress()`. `closing_report`
prints the rest once at the end of every call, and only the ones that are not
zero.

The network's own numbers, which are what the crate exists to produce:

| Counter | Non-zero means |
|---|---|
| `packets_received`, `packets_sent` | The baseline. At 20 ms packets these should each advance by 50 a second for as long as the call is up. Received stuck at 0 with sent advancing is a one-way-audio call: check `latched` and the Contact |
| `received` | The jitter buffer's own count of packets pushed into it, which on a healthy call is `packets_received`. The two coming apart would mean something between the reader and the buffer |
| `lost` | Packets that never arrived, counted when the stream moved past them. The network dropped them |
| `concealed` | Octets of silence handed to the modem in place of audio. This is the number. It is `lost` times the settled packet size, expressed in the units a capture is measured in, so a log and a capture agree on where the hole was and how long |
| `reordered` | Packets that overtook each other and were put back in place in time. A few is an ordinary home connection and costs nothing |
| `late` | Packets that arrived after the stream had already passed them, and could not be used. This is the cushion not being deep enough -- 40 ms of it, by default. A duplicate of a packet already handed over lands here too, so read it beside `duplicated` |
| `duplicated` | The same sequence number arriving twice while it was still waiting. Ordinary on a path with a retransmitting middlebox in it |
| `overflowed`, `overflowed_octets` | Packets shed because the jitter buffer filled, and the audio in them. The far end's clock runs faster than ours, or nothing was draining the buffer. The octets are there so this can be lined up against a capture the way `concealed` can |
| `resynced` | The far end's sequence jumped further forward than RFC 3550 A.1's `MAX_DROPOUT`: it restarted its stream rather than dropping part of one. The timeline stepped instead of filling minutes with silence. On a call that was not renegotiated, this means the far end's RTP source changed underneath us |
| `deepest` | The most packets ever waiting at once. A number well above 2 says how much jitter the path actually has |
| `latched` | The far end's RTP came from an address other than the one it advertised, and we followed it. Expected behind a router and not a fault -- but if audio is one-way, this is the first thing to look at, because it says whether symmetric RTP did its job |
| `strangers` | RTP from an address other than the call's stream that was not that stream moving, dropped rather than followed. Usually the last call's stream still arriving; a steady count on a call is somebody else sending to the port |
| `ignored` | Datagrams that arrived and were not RTP we could use. Scanners, or a middlebox mangling packets |
| `off_codec` | RTP that parsed but carried a payload type we never negotiated, dropped rather than decoded. Dialled digits sent as RFC 4733 events, comfort noise, or a transcoder that has changed codec under the call. Each one leaves a gap the buffer conceals, so `off_codec` and `concealed` climb together; a steady climb with no audio behind it is the third case and that call is over whatever we do |
| `payload_changed` | The payload type inside the stream the buffer accepted changed. It is kept because the buffer on its own cannot know what was agreed, but with the payload-type filter in `media.rs` in front of it nothing should now reach it that could move this. A non-zero value here is a fault in this crate rather than in the line |

And the numbers that are about this end rather than the line. They are worth as
much as the others, because a call blamed on a lost packet that was really
thrown away in here is a week of debugging in the wrong place:

| Counter | Non-zero means |
|---|---|
| `underruns` | Packets we sent as silence because the modem had not produced anything. A fault at *our* end: the modem loop is not keeping up, or is blocked. Exactly one packet of the call comes out of the primed queue, so on a healthy call this should stay at zero |
| `dropped_out` | Octets thrown off the outgoing queue because the pacer could not drain it. The modem is producing faster than real time, which means the loop is being driven wrongly |
| `flushed_out` | Octets taken out of the outgoing queue because the media path was started or stopped, rather than because it overflowed. At a disconnect it is the tail of the call and means nothing. During a call it means something restarted the path underneath it, and it is that much of the modem's own audio that never left this machine |
| `abandoned` | Octets that had arrived and were still waiting when the buffer was restarted: the cushion, plus anything behind it. At the end of a call it is the last few milliseconds. During one it is the same event as `flushed_out`, seen from the other direction, and it is that much audio the modem was owed and never got |

**The single most useful thing this crate adds to debugging is the first line of
that triage:** a call that failed with `concealed` at zero failed in the modem,
and a call that failed with `concealed` non-zero failed in the network. Under
the old path that distinction could not be drawn at all, because the softphone's
buffer concealed silently and by design -- which is how a 43 dB line came to be
recorded as a 17 dB one.

Beyond the counters, the first live call should confirm the things that have
only been tested against written-down descriptions: that the registrar accepts
the digest response, that `public` gets filled in from rport (a wrong one is
the usual reason a call connects and carries no audio), that the law negotiated
is the law the trunk actually sends, and that the codewords arrive unaltered --
which on a mu-law trunk can be checked directly, because this is the only path
where that question can even be asked.

TCP has its own list, and none of it has been seen against a real trunk either:
that a registration placed over a connection is answered down the same one,
that a trunk's inbound call arrives on the connection we already hold (18.2.1),
that a connection dropped mid-call is remade and the things in flight get
through on the second one, and that nothing is sent twice. The last is the
easiest to get wrong and the hardest to notice, because a duplicate over TCP
does not fail -- it is answered, separately, and the second answer is a 481 for
a transaction the far end has already finished with.

## What an adversarial read found, and what the tests could not see

The crate was written module by module against the documents, and when it was
finished it compiled and its 108 unit tests passed. An adversarial read of it
-- looking for what would go wrong on a real trunk rather than for what the
modules claimed -- found fifteen defects. Every one of them was fixed, and the
integration tests in `tests/call.rs` were written alongside the fixes.

The interesting part is not the list but the pattern. Each of the four below
was invisible for the same reason, and it is a reason that will recur.

**The duplicate 200 that would have fired on every call.** RFC 3261 13.3.1.4
has a user agent retransmit its 2xx to an INVITE every T1 until the ACK
arrives, because a 2xx is the one response with no transaction underneath it.
This path carries about 750 ms each way, so a far end always gets at least one
retransmission out before our ACK can reach it. Every 2xx was handled
identically: the dialog adopted again, the answer read again, another
`Answered` event -- which `line::Line::poll` turns into `Media::connect`, which
used to flush the jitter buffer and the outgoing queue. Half a second of the
modem's V.8 handshake, thrown away on every call. Nothing in the unit tests
could see it, because it is not a fault in any function: each of the four steps
is correct on its own, and the fault is that they ran twice. It needs two
messages and a clock to exist at all.

**The retransmitted INVITE that answered itself.** Being inside a dialog was
decided on the Call-ID alone, and the dialog was stored as soon as the call
started ringing here rather than when it was answered. So an ordinary
retransmission of a caller's INVITE -- not an edge case on a 750 ms path, but
the normal case -- reached the re-INVITE handler, which built an SDP answer and
sent 200 OK. The telephone answered itself, with no tag of ours on the To and
`ringing` still set as though nobody had picked up. Invisible for the same
reason again: every function was right, and the fault only exists when the same
message arrives twice -- which no test had ever made happen.

**The payload-type hole.** The reader filtered RTCP and nothing else, so an RFC
4733 telephone-event packet -- four octets -- took a sequence number and was
handed to the modem as four octets of G.711 where a hundred and sixty belonged.
Twenty milliseconds of the audio timeline deleted, per packet, uncounted, and a
dialled digit is ten to twenty of them. This one is worse than a bug in the
crate: it is the exact fault the crate was written to take out of the path,
reintroduced by omission. It was invisible because no test ever sent a packet
that was not the codec under test. Every test built its own audio, and a test
that builds its own input never sends the input that breaks it.

**The jitter cushion that was not there.** `target` was a priming threshold: on
the first drain the buffer waited for `target` packets, and on every drain
after that it emptied itself. Since the caller drains about ten times per
packet interval, the steady depth from the second drain onwards was one packet
or none, and the cushion this whole crate is about did not exist for any of the
call after its first moment. An ordinary pair-swap then produced a concealed
20 ms hole *and* a `late` packet -- a fault invented by the buffer and charged
to the network, which is worse than useless when `concealed` is the one number
the crate exists to produce. The tests passed because they pushed a handful of
packets and drained once. That is not how the caller uses it, and the
difference between "drain once" and "drain ten times per packet" is the whole
fault. The unit tests now drain the way `Media::receive` does, through a helper
that exists to make that impossible to forget.

The pattern in all four: **a module that is correct in isolation, in a
composition nothing ran.** Retransmission, ordering, a packet of a kind the
test did not write, and a caller whose cadence differs from the test's. That is
also a fair summary of which parts of SIP are dangerous to hand-roll. The
parsers and the hash are not -- they are fully specified, they are pure
functions, and a test against the document's own vectors settles them. What is
dangerous is everything with a timer or a duplicate in it: transaction
matching, the absorption of retransmissions, the 2xx that has to retransmit
itself, the CANCEL that must wait and then retransmit. Those cannot be read
against a clause and pronounced correct, because what they do is a sequence in
time rather than a function of their input.

So `tests/call.rs` runs them. A fake trunk lives in that file, on the loopback
with the system choosing its ports: it challenges the first REGISTER and the
first INVITE, checks the digest by computing it from RFC 2617's formula rather
than by asking the code under test what it meant, expects the ACK that 17.1.1.3
says a failure response gets, hangs up from its own end, sends an OPTIONS
keep-alive, asks for T.38, and sends the same BYE and the same re-INVITE twice
over. The agent registers, dials, comes up, carries audio through real sockets
and is put down from both ends. Every wait is a poll loop with a deadline that
says what it was waiting for and prints both transcripts when it gives up,
because a test of a running agent that ends in a bare assertion tells a reader
nothing about which of the dozen things in flight did not happen.

`transport.rs` was written after all this, and it is the first module in the
crate shaped by it. The TCP framing is the same kind of thing that went wrong
four times above -- a sequence, not a function; two messages in one read, a body
that arrives after its headers, a message with no body followed straight away
by another -- so it was deliberately written as a pure function over a buffer,
`frame`, which turns every one of those cases into an eight-line test that
needs no socket, no thread and no timing. Whether that was enough is a question
a live trunk will answer.

Two honest limits on all of this. These faults were found by reading and by a
trunk that exists only in a test file -- not by a live call, which has still
not happened. And a fake trunk is agreeable in ways a real one is not: it is on
the loopback, so nothing is reordered, nothing is lost, and the round trip is
microseconds where the real one is about 750 ms each way. It behaves the way
the RFC says because it was written from the same reading of the RFC that the
code was. A live call will find more.

## Status

The crate compiles and its 185 tests pass: 164 in the modules themselves and 21
in `tests/call.rs` against the fake trunk.

**It has placed live calls.** On 2026-09-23 it registered with an Australian
trunk over UDP, placed a call, and carried it in mu-law; the modem has since
worked over it. What that first call also produced was four faults that no
amount of reading had found -- a display that stopped refreshing at the exact
moment the thing it displayed stopped moving, a redial refused for up to
thirty-two seconds because a call being torn down still counted as a call, a
late answer to one call's BYE tearing down the next, and an outgoing cushion of
one packet that never recovered once spent. Every one of them is in this
document because a call was made, and none of them was findable without one.

Everything here about far-end behaviour that is not from that call is from the
RFC, from captures taken through the old softphone path, or from the module
written against it. TCP in particular has been exercised only against the fake
trunk; no live call has been placed over it.
