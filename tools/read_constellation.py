#!/usr/bin/env python3
"""Read a signal space diagram out of a Recommendation, by where the labels sit.

The extracted text of these figures cannot be trusted. A constellation is a
drawing: the binary labels and the axis numbers are text at particular places
on the page, and the lines and ticks between them are graphics. Extraction
flattens that into rows, loses the sign column of the negative axis labels, and
reorders anything that shares a line -- so a constellation read out of
`spec_text.sh` output is wrong in ways no test of our own two ends could
notice, because both ends would be wrong together.

So this reads the positions instead.

    python tools/read_constellation.py docs/specs/T-REC-V.32bis-199102-I.pdf \\
        --page 5 --bits 7 --rust

Two things are measured separately, because they are trustworthy for different
reasons:

* The **scale** comes from the axis tick numbers -- from the distances between
  them, not from where any one of them is. Eight ticks two units apart is an
  overdetermined measurement and the residual is printed, so a page where the
  numbers being read are not all ticks says so.

* The **origin** comes from the constellation itself. Every one of these is
  symmetric under a quarter turn, so the centroid of the labels is the origin
  exactly -- whereas a tick number sits beside its tick rather than on it, by
  an offset that differs between the two axes and is worth nothing.

What comes out is checked rather than assumed: every label must land on the
lattice, the codes must be a complete run, and the mean power and the parity of
x+y are printed for comparison with whatever the text of the Recommendation
does survive with.
"""

import argparse
import re
import sys
from collections import defaultdict

try:
    import fitz  # PyMuPDF
except ImportError:  # pragma: no cover - a tool, not part of the build
    sys.exit("this needs PyMuPDF: pip install pymupdf")


def spans(page):
    """Every run of text on the page, with the centre of its box."""
    out = []
    for block in page.get_text("dict")["blocks"]:
        for line in block.get("lines", []):
            for span in line["spans"]:
                text = span["text"].strip()
                if not text:
                    continue
                x0, y0, x1, y1 = span["bbox"]
                out.append((text, (x0 + x1) / 2, (y0 + y1) / 2))
    return out


def scale_from(ticks, axis):
    """Points on the page per unit of the diagram, from the tick spacing."""
    if len(ticks) < 2:
        return None
    n = len(ticks)
    sx = sum(v for v, _ in ticks)
    sy = sum(p for _, p in ticks)
    sxx = sum(v * v for v, _ in ticks)
    sxy = sum(v * p for v, p in ticks)
    slope = (n * sxy - sx * sy) / (n * sxx - sx * sx)
    intercept = (sy - slope * sx) / n
    worst = max(abs(slope * v + intercept - p) for v, p in ticks)
    print(f"  {axis} axis: {n} ticks, {abs(slope):.2f} pt per unit, "
          f"worst residual {worst:.2f} pt")
    return slope


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pdf")
    parser.add_argument("--page", type=int, required=True, help="0-based")
    parser.add_argument("--bits", type=int, required=True)
    parser.add_argument(
        "--pitch",
        type=float,
        help="units between adjacent points, when the figure labels too few "
        "ticks to measure a scale from. Measured off the labels themselves.",
    )
    parser.add_argument("--rust", action="store_true")
    parser.add_argument("--name", default="POINTS")
    args = parser.parse_args()

    page = fitz.open(args.pdf)[args.page]
    every = spans(page)
    numbers = [(t, x, y) for t, x, y in every if re.fullmatch(r"-?\d{1,2}", t)]
    labels = [
        (run, x, y)
        for t, x, y in every
        for run in t.split()
        if len(run) == args.bits and set(run) <= {"0", "1"}
    ]
    if not labels:
        sys.exit(f"no {args.bits}-bit labels on page {args.page}")

    # A page may hold two figures. The labels of each are told apart by their
    # bit length already; the tick numbers are not, so only those inside the
    # spread of these labels are this figure's.
    margin = 30.0
    left = min(x for _, x, _ in labels) - margin
    right = max(x for _, x, _ in labels) + margin
    top = min(y for _, _, y in labels) - margin
    bottom = max(y for _, _, y in labels) + margin
    numbers = [n for n in numbers if left < n[1] < right and top < n[2] < bottom]

    # The real axis runs along one row of the page and the imaginary along one
    # column, so the ticks of each cluster tightly on the other coordinate.
    def cluster(items, key):
        buckets = defaultdict(list)
        for item in items:
            buckets[round(item[key] / 6)].append(item)
        return max(buckets.values(), key=len)

    x_scale = scale_from(
        sorted((int(t), x) for t, x, _ in cluster(numbers, 2)), "real"
    )
    y_scale = scale_from(
        sorted((int(t), y) for t, _, y in cluster(numbers, 1)), "imaginary"
    )
    if x_scale is None or y_scale is None:
        if args.pitch is None:
            sys.exit(
                "too few ticks are labelled to measure a scale; give --pitch, "
                "the number of units between adjacent points"
            )
        # The smallest gap between two labels that are not on top of each other
        # is one step of the lattice, and the figure says how many units that
        # is. Measured over every pair so a single crooked label cannot set it.
        def pitch_of(values):
            # A row of labels is not drawn perfectly level -- one may sit two
            # or three points off its neighbours -- so anything closer than a
            # few points is one place on the lattice and is averaged first.
            rows = []
            for value in sorted(values):
                if rows and value - rows[-1][-1] < 8.0:
                    rows[-1].append(value)
                else:
                    rows.append([value])
            middles = [sum(r) / len(r) for r in rows]
            return min(b - a for a, b in zip(middles, middles[1:]))

        x_scale = x_scale or pitch_of({x for _, x, _ in labels}) / args.pitch
        y_scale = y_scale or -pitch_of({y for _, _, y in labels}) / args.pitch
        print(f"  scale from the label pitch: {abs(x_scale):.2f} and "
              f"{abs(y_scale):.2f} pt per unit")

    # The origin, from the constellation's own symmetry.
    x_origin = sum(x for _, x, _ in labels) / len(labels)
    y_origin = sum(y for _, _, y in labels) / len(labels)

    points = {}
    worst = 0.0
    for run, x, y in labels:
        u = (x - x_origin) / x_scale
        v = (y - y_origin) / y_scale
        worst = max(worst, abs(u - round(u)), abs(v - round(v)))
        code = int(run, 2)
        if code in points:
            print(f"  !! {run} appears twice")
        points[code] = (round(u), round(v))

    want = 1 << args.bits
    print(f"  {len(labels)} labels, {len(points)} distinct codes of {want}")
    print(f"  worst distance from the lattice: {worst:.2f} units")
    missing = [c for c in range(want) if c not in points]
    if missing:
        print(f"  !! missing {len(missing)}: "
              f"{[format(c, f'0{args.bits}b') for c in missing[:8]]}")
    if worst > 0.25:
        print("  !! a label did not land on the lattice; do not trust this")

    if len(points) == want:
        power = sum(x * x + y * y for x, y in points.values()) / want
        print(f"  mean power {power:.4f}")
        print(f"  x+y parity: {sorted({(x + y) % 2 for x, y in points.values()})}")
        # A constellation that is not closed under a quarter turn is one that
        # has been read crookedly.
        every_point = set(points.values())
        turned = {(-y, x) for x, y in every_point}
        print(f"  closed under a quarter turn: {turned == every_point}")

    if args.rust:
        print()
        width = max(len(f"{x}.0") for x, _ in points.values())
        print(f"const {args.name}: [(f64, f64); {want}] = [")
        for code in range(want):
            x, y = points.get(code, ("?", "?"))
            here = f"({x}.0, {y}.0),".ljust(width * 2 + 6)
            print(f"    {here} // {format(code, f'0{args.bits}b')}")
        print("];")


if __name__ == "__main__":
    main()
