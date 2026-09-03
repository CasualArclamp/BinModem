"""Render a spectrogram of a modem capture with segment boundaries annotated."""
from __future__ import annotations
import sys
import numpy as np
import matplotlib; matplotlib.use("Agg")
import matplotlib.pyplot as plt
from scipy.signal import resample_poly, spectrogram
sys.path.insert(0, "tools")
from analyze_capture import read_wav_raw, segments

def main(path, out, t0=None, t1=None, nfft=1024):
    x, fs0 = read_wav_raw(path)
    mono = x.mean(axis=1)
    if t0 is not None:
        mono = mono[int(t0*fs0):int(t1*fs0)]
    off = t0 or 0.0
    m = resample_poly(mono, 80, 441); fs = 8000          # 44100 -> 8000
    f, t, S = spectrogram(m, fs, nperseg=nfft, noverlap=nfft*3//4,
                          window="hann", scaling="spectrum")
    S = 10*np.log10(S + 1e-12)
    fig, ax = plt.subplots(figsize=(22, 6), dpi=110)
    ax.pcolormesh(t+off, f, S, shading="gouraud", cmap="turbo",
                  vmin=np.percentile(S, 55), vmax=np.percentile(S, 99.9))
    ax.set_ylim(0, 4000); ax.set_ylabel("Hz"); ax.set_xlabel("seconds")
    ax.set_title(f"{path}  ({len(m)/fs:.1f}s @ {fs} Hz)")
    for g in (1070, 1270, 1650, 1800, 1850, 2025, 2100, 2225, 2400):
        ax.axhline(g, color="w", lw=0.35, alpha=0.30)
    if t0 is None:
        for i, (a, b) in enumerate(segments(mono, fs0)):
            ax.axvline(a/fs0, color="w", lw=1.4)
            ax.axvline(b/fs0, color="w", lw=1.4, ls=":")
            ax.text(a/fs0+0.15, 3780, f"#{i}", color="w", fontsize=11, weight="bold")
    fig.tight_layout(); fig.savefig(out)
    print(f"wrote {out}")

if __name__ == "__main__":
    a = sys.argv[1:]
    main("WAV/ALL Old Modem Sounds (300 baud to 56K).wav",
         a[0] if a else "captures/overview.png",
         float(a[1]) if len(a) > 2 else None,
         float(a[2]) if len(a) > 2 else None)
