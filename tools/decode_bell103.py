"""Bell 103 receiver: FM discriminator + carrier gate + async 8N1 framer.

Bell 103 is frequency-division full duplex:
    originate  space 1070 Hz   mark 1270 Hz
    answer     space 2025 Hz   mark 2225 Hz

Tone spacing is 200 Hz at 300 baud (modulation index h=0.67), so the two tones
are NOT orthogonal and a dual-tone energy detector bleeds badly. A frequency
discriminator is the correct demodulator. A direct line tap carries both
directions summed, so each band is bandpass-isolated before detection.
"""
from __future__ import annotations
import sys
import numpy as np
from scipy.signal import butter, sosfiltfilt, resample_poly, hilbert
sys.path.insert(0, "tools")
from analyze_capture import read_wav_raw

BAUD = 300.0
BANDS = {
    "ORIGINATE (caller -> host)": (1070.0, 1270.0),
    "ANSWER    (host -> caller)": (2025.0, 2225.0),
}

def discriminate(x, fs, f_space, f_mark, baud=BAUD):
    """Return (normalised frequency estimate, carrier-present mask).
    +1 => mark(1), -1 => space(0)."""
    fc, dev = (f_space + f_mark) / 2.0, (f_mark - f_space) / 2.0
    bp = butter(6, [fc - 320, fc + 320], btype="band", fs=fs, output="sos")
    xb = sosfiltfilt(bp, x)

    n = np.arange(len(xb))
    z = hilbert(xb) * np.exp(-2j * np.pi * fc * n / fs)
    lp = butter(4, 380, btype="low", fs=fs, output="sos")
    z = sosfiltfilt(lp, z.real) + 1j * sosfiltfilt(lp, z.imag)

    inst = np.angle(z[1:] * np.conj(z[:-1])) * fs / (2 * np.pi)
    inst = np.concatenate([[inst[0]], inst])
    pd = butter(3, baud * 0.8, btype="low", fs=fs, output="sos")
    d = sosfiltfilt(pd, inst) / dev

    env = np.abs(z)
    carrier = env > (0.25 * np.median(env[env > np.percentile(env, 60)]))
    return d, carrier

def frame_async(d, carrier, fs, baud=BAUD, bits=8):
    """8N1 async framing. Idle is mark; require a settled mark before accepting
    a start bit, sample each bit at its centre, and validate the stop bit."""
    sp = fs / baud
    out, i, n = [], int(sp), len(d)
    limit = n - int(sp * (bits + 3))
    while i < limit:
        if not carrier[i]:
            i += 1; continue
        # start bit = mark -> space transition, with the line previously idle
        if d[i-1] > 0 >= d[i] and d[i - int(sp*0.5)] > 0:
            if d[i + int(sp*0.5)] > 0:          # too short: glitch, not a start bit
                i += 1; continue
            val = 0
            for b in range(bits):               # LSB first
                val |= (1 if d[int(i + sp*(1.5+b))] > 0 else 0) << b
            if d[int(i + sp*(1.5+bits))] > 0:   # stop bit must be mark
                out.append(val)
                i += int(sp * (bits + 1.5)); continue
        i += 1
    return bytes(out)

def render(b):
    return "".join(chr(c) if 32 <= c < 127 else
                   {10: "\n      ", 13: "", 9: "\t"}.get(c, f"<{c:02X}>") for c in b)

def main(path, t0, t1):
    x, fs0 = read_wav_raw(path)
    seg = x.mean(axis=1)[int(t0*fs0):int(t1*fs0)]
    mono = resample_poly(seg, 160, 441); fs = 16000      # 44100 -> 16000
    print(f"segment {t0:.2f}-{t1:.2f}s  ({len(mono)/fs:.2f}s @ {fs} Hz)\n")
    for label, (fsp, fmk) in BANDS.items():
        d, car = discriminate(mono, fs, fsp, fmk)
        data = frame_async(d, car, fs)
        print(f"--- {label}  [{fsp:.0f}/{fmk:.0f} Hz]  {len(data)} chars, "
              f"carrier {100*car.mean():.0f}% of segment")
        print(f"      {render(data)}\n")

if __name__ == "__main__":
    main("WAV/ALL Old Modem Sounds (300 baud to 56K).wav",
         float(sys.argv[1]) if len(sys.argv) > 1 else 4.0,
         float(sys.argv[2]) if len(sys.argv) > 2 else 16.5)
