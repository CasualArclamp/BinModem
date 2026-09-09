"""Read the far end's V.32 rate signals straight off a recording.

Only for looking at a call that went wrong. The modem's own replay is the
better tool when it works; this exists for when it does not, and it makes no
decisions -- it prints the sixteen bits and what they say.
"""
import sys
import wave
import numpy as np

path = sys.argv[1]
w = wave.open(path)
raw = np.frombuffer(w.readframes(w.getnframes()), dtype="<i2").astype(np.float64) / 32768.0
h = raw[0::2]
fs = float(w.getframerate())
BAUD = 2400.0
CARRIER = 1800.0
STATES = np.array([-3 - 1j, 1 - 3j, 3 + 1j, -1 + 3j])   # A B C D; index = quadrant
CHANGE_TO_DIBIT = [0b01, 0b00, 0b10, 0b11]


def lowpass(x, cutoff, taps=193):
    n = np.arange(taps) - (taps - 1) / 2
    hh = np.sinc(2 * cutoff / fs * n) * np.hamming(taps)
    return np.convolve(x, hh / hh.sum(), "same")


def bits_from(a, b):
    x = h[int(a * fs):int(b * fs)]
    t = np.arange(len(x)) / fs
    bb = lowpass(x * np.exp(-2j * np.pi * CARRIER * t), 1500.0)
    # Residual carrier offset: a four-point constellation raised to the fourth
    # power leaves a line at four times whatever it is off by.
    p4 = np.asarray(bb, dtype=np.complex128) ** 4
    sp = np.abs(np.fft.fft(p4))
    f = np.fft.fftfreq(len(p4), 1 / fs)
    off = f[int(np.argmax(sp))] / 4.0
    bb = bb * np.exp(-2j * np.pi * off * np.arange(len(bb)) / fs)
    sps = fs / BAUD
    best, bestv = 0, -1.0
    for o in range(int(sps)):
        idx = (np.arange(int((len(bb) - o) / sps)) * sps + o).astype(int)
        v = float(np.abs(bb[idx]).mean())
        if v > bestv:
            best, bestv = o, v
    idx = (np.arange(int((len(bb) - best) / sps)) * sps + best).astype(int)
    sym = bb[idx]
    # Any fixed rotation is harmless: the coding is differential.
    q = np.argmin(np.abs(sym[:, None] - STATES[None, :]), axis=1)
    ch = (q[1:] - q[:-1]) % 4
    out = []
    for c in ch:
        d = CHANGE_TO_DIBIT[c]
        out += [(d >> 1) & 1, d & 1]
    return np.array(out, dtype=np.uint8), off


def descramble(bits, taps):
    a, b = taps
    out = np.zeros(len(bits), dtype=np.uint8)
    for i in range(len(bits)):
        v = int(bits[i])
        if i >= a:
            v ^= int(bits[i - a])
        if i >= b:
            v ^= int(bits[i - b])
        out[i] = v
    return out


def sync(s):
    return (s & 0xF000) == 0 and (s >> 7) & 1 and (s >> 4) & 1 and s & 1


def endsync(s):
    return (s & 0xF000) == 0xF000 and (s >> 7) & 1 and (s >> 4) & 1 and s & 1


def show(a, b, label):
    bits, off = bits_from(a, b)
    d = descramble(bits, (5, 23))       # the answering modem's polynomial (4.1)
    seen = {}
    for i in range(len(d) - 16):
        v = 0
        for k in range(16):
            v = (v << 1) | int(d[i + k])
        if sync(v) or endsync(v):
            seen[v] = seen.get(v, 0) + 1
    # Runs at the locked phase: what a detector counting consecutive
    # identical sequences would actually see.
    phase = None
    best = {}
    for start in range(16):
        seqs = []
        for i in range(start, len(d) - 16, 16):
            v = 0
            for k in range(16):
                v = (v << 1) | int(d[i + k])
            seqs.append(v)
        good = sum(1 for v in seqs if sync(v))
        if phase is None or good > phase[1]:
            phase = (start, good, seqs)
    runs = {}
    cur, n = None, 0
    for v in phase[2]:
        if v == cur:
            n += 1
        else:
            if cur is not None and sync(cur):
                runs[cur] = max(runs.get(cur, 0), n)
            cur, n = v, 1
    if cur is not None and sync(cur):
        runs[cur] = max(runs.get(cur, 0), n)
    print(f"{label}: offset {off:+.1f} Hz, {len(bits)} bits")
    print("   longest run of identical sequences at the locked phase:")
    for v, n in sorted(runs.items(), key=lambda kv: -kv[1])[:5]:
        print("     {:3d} x {:016b}".format(n, v))
    if not seen:
        print("   nothing with the synchronising bits of 5.3.1")
    for v, n in sorted(seen.items(), key=lambda kv: -kv[1])[:4]:
        bit = lambda k: (v >> (15 - k)) & 1
        v32bis = bit(4) and bit(8)
        if v32bis:
            rates = [r for r, k in [(14400, 12), (12000, 10), (9600, 6), (7200, 9), (4800, 5)] if bit(k)]
        else:
            rates = [r for r, k in [(9600, 6), (4800, 5), (2400, 4)] if bit(k)]
        print("   seen {:4d}  {} {:016b}  {}  {}{}".format(
            n, "E" if endsync(v) else "R", v,
            "V.32bis" if v32bis else "V.32   ",
            " ".join(map(str, rates)) or "none (clear down)",
            "" if v32bis else (", trellis" if bit(8) else ""),
        ))
    print()


for a, b, label in [
    (22.3, 24.6, "far end, its first rate signal   (22.3-24.6 s)"),
    (27.7, 33.1, "far end, after its first restart (27.7-33.1 s)"),
    (37.0, 39.4, "far end, after the retrain       (37.0-39.4 s)"),
]:
    show(a, b, label)
