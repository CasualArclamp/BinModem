# Fax

[Back to the README](../../README.md)

BinModem is also a fax machine. [Sending a fax](../usage.md#sending-a-fax)
gives the procedure.

## Pages

A picture that you load in the Fax window becomes a group 3 page. A PDF
becomes one page for each of its pages, at their printed size. The pages go
out under T.30.

A public fax service over the same VoIP trunk accepts the training check and
confirms the page. A page that the service sends back arrives, and you can
save it as a PDF or a PNG.

## V.29 and V.27 ter

V.29 carries the page at 9600 and 7200. It falls back to V.27 ter at 4800 and
2400. Every point and training sequence of both was read from the figures. The
extracted text changes the square root of two into "2".

One change let the page of a real machine in. The transmitter of that machine
puts twenty milliseconds of silence before every training sequence on purpose.
BinModem had read that silence as the end of the burst.

## V.17

V.17 carries the page at 14 400 down to 7200. It works between two BinModems
and with real fax machines.

## Super G3

Super G3 puts V.8 before the call. It sends the page over the half-duplex mode
of V.34 at up to 33 600. The frames of T.30 go on its 1200 bit/s control
channel.

Super G3 also works between two BinModems, over the delay and the slips of the
VoIP line. A real Super G3 fax has sent a page to a BinModem at 33 600 on 3429
baud, in JBIG. A transmission to a real Super G3 fax was also successful.

## Error correction mode

Between two BinModems the page goes under the error correction mode of T.30.
That mode uses numbered frames. It sends a partial page request for each frame
that the far end could not read.

A tenth of a second of loud noise on a page costs a few frames that are sent
again. The page arrives exactly as it left. Without error correction the same
noise prints as streaks.

Error correction also lets the page go in MMR, the code of T.6. MMR has no
end-of-line codes with which to find its place again. Its size is less than
half of the Modified Huffman that every machine reads.

BinModem draws a page one row at a time as it arrives, as slow-scan television
does.

## Colour and greyscale

BinModem sends and receives greyscale and colour pages. A page goes as T.4
Annex E gives it: a baseline JPEG data stream with CIELAB components, in error
correction mode, at 200 pels to 25.4 mm.

To receive:

- When BinModem answers, its DIS has bit 68 (JPEG coding) and bit 69 (full
  colour mode) of T.30 Table 2. The **Colour** tick box in the Fax window sets
  this offer. It needs **ECM**.
- The page shows in colour as it arrives, with a **JPEG colour** or
  **JPEG grey** badge.
- **Save** writes a PNG in sRGB, or a PDF that contains the JPEG stream as it
  arrived.

To send:

- Load a picture or a PDF. In the **tone** row, select **greyscale** or
  **colour**. The page view then shows the page as it goes.
- The **quality** row sets the size of the JPEG: **small**, **normal** or
  **best**. The size and the time at the fastest rate show beside it.
- The pages go in JPEG only if the DIS of the far end has bit 68. Colour also
  needs bit 69. A far end with bit 68 only gets the pages in greyscale.
- A far end without bit 68 gets the pages of ink, as the **ink** row sets them.
- The **Colour** and **ECM** tick boxes must have a tick.

Colour is tested between two BinModem ends in loopback, not with a real colour
fax machine.
[The colour fax specification](../design/colour-fax/spec.md) lists the clauses.
