"""Draw the constellation pictures from the points that the example writes.

    cargo run --release -p modem --example constellations -- docs/img/constellations
    python tools/constellations.py docs/img/constellations

Each folder with a points.txt gives <folder>.png beside it, and all of them
together give overview.png. The points files are then deleted, because the
pictures are what the docs use.
"""

import math
import pathlib
import sys

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402
import numpy as np  # noqa: E402

GREEN = "#3f3"


def load(folder):
    points = np.loadtxt(folder / "points.txt", ndmin=2)
    title, noise, peak, order = (folder / "label.txt").read_text(encoding="utf-8").splitlines()[:4]
    return points, title, noise, float(peak), int(order)


def draw(ax, points, title, noise, peak, small=False):
    ax.set_facecolor("black")
    size = 0.5 if len(points) > 6000 else 1.5
    ax.scatter(points[:, 0], points[:, 1], s=size if not small else size / 2, c=GREEN, linewidths=0)
    reach = np.percentile(np.abs(points), 99.8) if len(points) else 1.0
    lim = 1.1 * max(reach, min(peak, 2 * reach))
    ax.set_xlim(-lim, lim)
    ax.set_ylim(-lim, lim)
    ax.set_aspect("equal")
    ax.set_title(f"{title}\n{noise}", color="w", fontsize=8 if small else 11)
    ax.tick_params(colors="gray", labelsize=6 if small else 8)
    if small:
        ax.set_xticks([])
        ax.set_yticks([])
    for spine in ax.spines.values():
        spine.set_color("#333")


def main():
    out = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "docs/img/constellations")
    folders = sorted(p for p in out.iterdir() if (p / "points.txt").exists())
    loaded = sorted(((f.name, *load(f)) for f in folders), key=lambda row: row[-1])
    loaded = [row[:-1] for row in loaded]
    for name, points, title, noise, peak in loaded:
        fig, ax = plt.subplots(figsize=(6, 6.4), facecolor="black")
        draw(ax, points, title, noise, peak)
        fig.tight_layout()
        fig.savefig(out / f"{name}.png", dpi=110, facecolor="black")
        plt.close(fig)
    cols = 4
    rows = math.ceil(len(loaded) / cols)
    fig, axes = plt.subplots(rows, cols, figsize=(3.2 * cols, 3.5 * rows), facecolor="black")
    for ax in np.ravel(axes):
        ax.set_visible(False)
    for ax, (name, points, title, noise, peak) in zip(np.ravel(axes), loaded):
        ax.set_visible(True)
        draw(ax, points, title, noise, peak, small=True)
    fig.tight_layout()
    fig.savefig(out / "overview.png", dpi=100, facecolor="black")
    plt.close(fig)
    for folder in folders:
        for f in folder.iterdir():
            f.unlink()
        folder.rmdir()


if __name__ == "__main__":
    main()
