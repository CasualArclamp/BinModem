"""Cut the reference capture into per-standard test vectors (16 kHz mono).

16 kHz preserves the whole voiceband with large margin (V.34's widest signal
tops out near 3.7 kHz) and decimates cleanly to the 8 kHz / 9.6 kHz rates a
datapump actually runs at.
"""
from __future__ import annotations
import sys, struct
import numpy as np
from scipy.signal import resample_poly
sys.path.insert(0, "tools")
from analyze_capture import read_wav_raw

SRC = "WAV/ALL Old Modem Sounds (300 baud to 56K).wav"
OUT = "tests/vectors"
FS  = 16000

# Generous margins so each vector contains the full call: pre-carrier silence,
# answer tone, negotiation, training, data and disconnect.
CALLS = [
    ("bell103-300",    3.8,  16.6, "Bell 103 300 bps FSK; carries the login session"),
    ("v22bis-2400",   17.9,  34.8, "ITU-T V.22bis 2400 bps, FDM full duplex"),
    ("v32bis-14400",  37.3,  55.4, "ITU-T V.32bis 14400 bps, echo-cancelled"),
    ("v34-33600",     57.5,  75.4, "ITU-T V.34 33600 bps, V.8 + line probing"),
    ("v90-56k",       77.7, 102.4, "ITU-T V.90 56k, V.34-style startup"),
    ("v92-56k",      103.2, 126.5, "ITU-T V.92 56k, V.34-style startup"),
]

def write_wav(path, x, fs):
    pcm = np.clip(x, -1.0, 1.0)
    pcm = (pcm * 32767.0).astype("<i2").tobytes()
    with open(path, "wb") as f:
        f.write(b"RIFF" + struct.pack("<I", 36+len(pcm)) + b"WAVE")
        f.write(b"fmt " + struct.pack("<IHHIIHH", 16, 1, 1, fs, fs*2, 2, 16))
        f.write(b"data" + struct.pack("<I", len(pcm)) + pcm)

def main():
    x, fs0 = read_wav_raw(SRC)
    mono = x.mean(axis=1)
    lines = ["# Golden test vectors", "",
             f"Cut from `{SRC}` — a direct line capture of a ~2005 Conexant V.92",
             "softmodem forced to each modulation with `AT+MS`. Both directions are",
             "summed on one tap, as on a real 2-wire line.", "",
             f"Resampled {fs0} Hz stereo -> {FS} Hz mono.", "",
             "| file | source window | duration | contents |",
             "|---|---|---|---|"]
    for name, t0, t1, desc in CALLS:
        seg = mono[int(t0*fs0):int(t1*fs0)]
        y = resample_poly(seg, FS//100, fs0//100)     # 44100 -> 16000 (160:441)
        peak = np.abs(y).max()
        path = f"{OUT}/{name}.wav"
        write_wav(path, y, FS)
        print(f"  {name:<16} {t1-t0:6.2f}s  peak {20*np.log10(peak+1e-12):6.1f} dBFS  -> {path}")
        lines.append(f"| `{name}.wav` | {t0:.2f}–{t1:.2f} s | {t1-t0:.2f} s | {desc} |")
    open(f"{OUT}/README.md", "w").write("\n".join(lines) + "\n")
    print(f"\nwrote {OUT}/README.md")

if __name__ == "__main__":
    main()
