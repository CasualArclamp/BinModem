# Colour fax: the clauses BinModem builds against

This page lists each clause, table and constant that the colour fax code uses.
The editions are T.30 (09/2005), T.4 (07/2003), T.42 (07/2003) and T.81 (09/1992).
`tools/fetch_specs.sh` fetches the three T-series texts. T.81 is not on the ITU
free list. A public copy is at <https://www.w3.org/Graphics/JPEG/itu-t81.pdf>.

The scope of this step is reception only. BinModem answers, offers colour in its
DIS, and receives a greyscale or colour page.

## 1. Negotiation: T.30 Table 2 and Annex E

| Bit | DIS/DTC | DCS | Note | BinModem |
|---|---|---|---|---|
| 15 | 200 x 200 pels/25.4 mm (also 7.7 lines/mm) | the page uses it | 25, 34 | always set in the DIS |
| 27 | error correction mode | ECM in use | 25 | necessary for bit 68 |
| 36 | T.43 coding | T.43 in use | 25 | not offered |
| 68 | JPEG coding | the page is JPEG | 25, 34 | offered with ECM |
| 69 | full colour mode | the page has L\*, a\* and b\* | 35 | offered with ECM |
| 70 | set to 0 | preferred Huffman tables | 36 | ignored: the tables come in the stream |
| 71 | 12 bits/pel component | 12 bits in use | 37 | not offered |
| 72 | extend field | extend field | 5 | set when the field goes on |
| 73 | no subsampling (1:1:1) | no subsampling in use | 38 | not offered, but the decoder reads it |
| 74 | custom illuminant | custom illuminant in use | 39 | not offered |
| 75 | custom gamut range | custom gamut in use | 40 | not offered |

Rules:

- Note 25: a DIS with bit 68 must also have bits 15 and 27 set.
- Note 34: a DCS with bit 68 says that the page is JPEG. It must also have
  bit 27 and a resolution bit set (15 for 200 x 200).
- Note 35: bit 68 without bit 69 is greyscale mode: the stream has only L\*.
- Note 36: the Huffman tables are always in the stream (Annex E.4: "The
  transmission of Huffman tables is mandatory").
- Note 38: with bit 73 clear, a\* and b\* have one sample for each four L\*
  samples. T.30 and T.4 call this "4:1:1". It is 2 x 2 subsampling (H = 2,
  V = 2 for L\*).
- Note 5: bit 68 is in octet 9. The extend bits 24, 32, 40, 48, 56 and 64 must
  be set for the field to get there.
- Table E.1/T.30: the mandatory set is 8 bits a component, 4:1:1 subsampling,
  illuminant D50, the default gamut range and 200 x 200 pels/25.4 mm. That set
  is what BinModem offers.

## 2. The data stream: T.4 Annex E

- E.1, E.6.1: the page goes only in ECM. Pad octets X'00' can follow EOI in the
  last ECM frame.
- E.2.3: baseline JPEG, sequential DCT, Huffman coding, 8 bits a sample.
- E.6.2: marker classes.
  - Always present: SOI, APP1, DQT, DHT, SOF0, SOS, EOI.
  - Optional, and the decoder must obey them: DRI, RSTn, DNL.
  - Optional, and the decoder must skip them: COM, other APPn.
  - SOF1 needs negotiation. BinModem reads SOF1 only when its precision is 8.
- E.6.2: the samples in a line, X, are a width from T.4 clause 2. BinModem
  offers only 215 mm, which is 1728 pels at 200 pels/25.4 mm.
- E.6.2: if Y in the frame header is 0, the DNL marker after the scan gives the
  number of lines. DNL can also make Y smaller.
- E.6.2.1: component 0 is L\* (H = 2, V = 2, tables 0), component 1 is a\* and
  component 2 is b\* (H = 1, V = 1, tables 1). There is one interleaved scan.
- E.6.2.2: an MCU is L\*, L\*, L\*, L\*, a\*, b\*. A greyscale page has only L\*.
- E.6.3: each chrominance sample is the mean of four values, and its centre is
  between the four L\* pels (Figure E.1).
- E.6.5: `APP1` directly after SOI: `"G3FAX"`, X'00', the version X'07CA' and
  the resolution in pels/25.4 mm (X'00C8' is 200).
- E.6.6: `APP1` with `"G3FAX"`, X'01': a custom gamut range, six signed 16-bit
  integers (offset and range for L\*, a\* and b\*). BinModem reads it.
- E.6.7: `APP1` with `"G3FAX"`, X'02': the illuminant, four octets. BinModem
  reads it, and uses D50 in all cases.
- E.6.9: each octet goes on the line with its least significant bit first.

## 3. The colour space: T.42 and T.4 E.6.4

- CIELAB with the D50 white point X0 = 96.422, Y0 = 100.000, Z0 = 82.521.
- The default gamut range is L\* = [0, 100], a\* = [-85, 85], b\* = [-75, 125].
- The 8-bit code is:
  - L = L\* x 255/100
  - a = a\* x 255/170 + 128
  - b = b\* x 255/200 + 96
- With a custom gamut (E.6.6), the code is N = (255/Q) x n\* + P, with the
  offset P and the range Q from the marker.

The picture stays in these codes. The window changes them to sRGB only to show
them or to write a PNG: CIELAB to XYZ (D50), a Bradford change to D65, then the
sRGB matrix and curve (IEC 61966-2-1).

## 4. JPEG: T.81

| Subject | Clause |
|---|---|
| Markers and segment syntax | B.1.1, Table B.1 |
| Frame header (SOF0) | B.2.2 |
| Scan header (SOS) | B.2.3 |
| Quantisation tables (DQT) | B.2.4.1 |
| Huffman tables (DHT) | B.2.4.2 |
| Restart interval (DRI) | B.2.4.4 |
| Number of lines (DNL) | B.2.5 |
| MCU order and interleave | A.2.3 |
| Zig-zag order | Figure A.6 |
| FDCT and IDCT | A.3.3 |
| Level shift | A.3.1 |
| Huffman code tables from BITS and HUFFVAL | C.2, Figures C.1 to C.3 |
| DC decode, EXTEND | F.2.2.1, Figure F.12 |
| AC decode | F.2.2.2, Figure F.13 |
| DECODE | F.2.2.3, Figures F.15, F.16 |
| Byte stuffing, restart | F.1.2.3, F.2.2.5, E.2.4 |
| Example tables (also the "preferred" tables of T.30 Note 36) | K.1 to K.6 |

## 5. The file: PDF

- ISO 32000-1 7.4.8: `DCTDecode` takes the JPEG stream as it arrived.
- ISO 32000-1 8.6.5.4: the `Lab` colour space with `WhitePoint` and `Range`.
- A PDF reader does not obey DNL. If Y was 0, the saved stream gets the true Y
  in its frame header, and the DNL segment is removed. No other octet changes.

## 6. Not in this step

- Colour transmission from the window.
- T.43 (bit 36), T.44 MRC (bits 92 to 94), sYCC (bit 127).
- 12 bits a component (bit 71), a custom illuminant (bit 74), a custom gamut
  offer (bit 75), resolutions other than 200 x 200.
