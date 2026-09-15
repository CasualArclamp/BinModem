#!/usr/bin/env python3
"""Draw a V.34 capture's received points as constellations, one panel a window.

The scope in the window keeps a few seconds and is a couple of hundred pixels
across, which is plenty for sixteen points and not for data mode's hundreds.
This draws them from a capture instead, as a density: every symbol in the
window goes into a fine grid of bins and the darker a bin the more often it was
hit, so each point of an 832-point constellation is a spot and what lies
between spots is noise.

First dump the points. The capture decoder in the data pump's tests writes
every equalised symbol between two times -- in data mode in its grid units,
where the points sit on odd coordinates:

    V34_CAPTURE=dist/captures/live-1789442205.wav V34_CHANNEL=0 \\
        V34_SENDER=answer V34_FROM=13 V34_HIGH=1 V34_DATA_RATE=31200 \\
        V34_DUMP=points.csv V34_DUMP_FROM=13 V34_DUMP_TO=40 \\
        cargo test --release -p datapump --test v34_capture captured_end \\
        -- --ignored --nocapture

Then draw whichever stretches of it are wanted:

    python tools/plot_constellation.py points.csv --window 21.53:27.45 \\
        --window 27.49:28.84 --out constellations.png

A window draws data mode's grid units if it holds any, and otherwise the
training symbols at unit power.
"""

import argparse
import math

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np
from matplotlib.colors import LinearSegmentedColormap, LogNorm

SURFACE = "#fcfcfb"
INK = "#0b0b0b"
SECONDARY = "#52514e"
MUTED = "#898781"
HAIRLINE = "#e1e0d9"
# One hue, light to dark: how often, not which.
RAMP = ["#cde2fb", "#9ec5f4", "#6da7ec", "#3987e5", "#256abf", "#184f95", "#0d366b"]


def window(text):
    start, end = text.split(":")
    return float(start), float(end)


def panel(ax, rows, start, end):
    chosen = rows[(rows[:, 0] >= start) & (rows[:, 0] < end)]
    grid = chosen[chosen[:, 4] == 1]
    points = grid if len(grid) else chosen[chosen[:, 4] == 0]
    units = "grid units" if len(grid) else "unit power"
    ax.set_facecolor(SURFACE)
    for side in ax.spines.values():
        side.set_color(HAIRLINE)
    ax.tick_params(colors=MUTED, labelsize=7, length=2)
    if not len(points):
        ax.set_title(f"{start:.2f} to {end:.2f} s: nothing", loc="left", color=SECONDARY, fontsize=9)
        return
    re, im = points[:, 1], points[:, 2]
    reach = float(np.percentile(np.maximum(np.abs(re), np.abs(im)), 99.9)) * 1.08
    # Bins a tenth of the way between neighbouring points: a spot a point.
    size = 0.2 if units == "grid units" else 0.01
    bins = max(64, int(2 * reach / size))
    counts, xs, ys = np.histogram2d(re, im, bins=bins, range=[[-reach, reach], [-reach, reach]])
    counts = np.ma.masked_where(counts == 0, counts)
    shades = LinearSegmentedColormap.from_list("often", RAMP)
    shades.set_bad(SURFACE)
    ax.pcolormesh(xs, ys, counts.T, cmap=shades, norm=LogNorm(vmin=1, vmax=max(2, counts.max())), shading="auto", rasterized=True)
    ax.axhline(0, color=HAIRLINE, lw=0.6, zorder=0)
    ax.axvline(0, color=HAIRLINE, lw=0.6, zorder=0)
    ax.set_xlim(-reach, reach)
    ax.set_ylim(-reach, reach)
    ax.set_aspect("equal")
    ax.set_title(f"{start:.2f} to {end:.2f} s, {len(points):,} symbols, {units}", loc="left", color=SECONDARY, fontsize=9)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("dump", help="time,re,im,error,data rows from tests/v34_capture.rs")
    parser.add_argument("--window", type=window, action="append", required=True, metavar="FROM:TO", help="seconds; give it again for another panel")
    parser.add_argument("--out", default="constellations.png")
    parser.add_argument("--title", default="Received constellations")
    args = parser.parse_args()

    rows = np.loadtxt(args.dump, delimiter=",", ndmin=2)
    if rows.shape[1] < 5:
        # An older dump, before data mode was written: training symbols only.
        rows = np.hstack([rows, np.zeros((len(rows), 5 - rows.shape[1]))])
    count = len(args.window)
    columns = min(count, 2)
    lines = math.ceil(count / columns)
    fig, axes = plt.subplots(lines, columns, figsize=(6.2 * columns, 6.5 * lines), dpi=140, squeeze=False)
    fig.patch.set_facecolor(SURFACE)
    for ax, (start, end) in zip(axes.flat, args.window):
        panel(ax, rows, start, end)
    for ax in list(axes.flat)[count:]:
        ax.set_visible(False)
    fig.suptitle(args.title, x=0.02, ha="left", color=INK, fontsize=12, fontweight="bold")
    fig.tight_layout()
    fig.savefig(args.out, facecolor=SURFACE)
    print(f"{args.out}: {count} panel{'s' if count != 1 else ''}")


if __name__ == "__main__":
    main()
