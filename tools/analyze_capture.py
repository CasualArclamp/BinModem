"""Inspect a modem-audio capture: repair broken headers, segment on silence,
and fingerprint each segment against known handshake tones."""
from __future__ import annotations
import struct, sys
import numpy as np

def read_wav_raw(path):
    """Read a WAV even when the data-chunk size is bogus (streaming writers
    stamp 0x3FFFFFFF). Falls back to 'rest of file'."""
    with open(path, "rb") as f:
        blob = f.read()
    assert blob[:4] == b"RIFF" and blob[8:12] == b"WAVE", "not a RIFF/WAVE file"
    pos, fmt = 12, None
    while pos + 8 <= len(blob):
        cid = blob[pos:pos+4]; csz = struct.unpack("<I", blob[pos+4:pos+8])[0]
        body = pos + 8
        if cid == b"fmt ":
            tag, ch, sr, _br, _ba, bits = struct.unpack("<HHIIHH", blob[body:body+16])
            fmt = dict(tag=tag, ch=ch, sr=sr, bits=bits)
        elif cid == b"data":
            avail = len(blob) - body
            n = avail if (csz == 0 or csz > avail) else csz
            raw = blob[body:body+n]
            break
        pos = body + csz + (csz & 1)
    else:
        raise ValueError("no data chunk")
    assert fmt and fmt["bits"] == 16, f"expected 16-bit, got {fmt}"
    x = np.frombuffer(raw[: (len(raw)//(2*fmt['ch']))*2*fmt['ch']], dtype="<i2")
    x = x.reshape(-1, fmt["ch"]).astype(np.float64) / 32768.0
    return x, fmt["sr"]

def segments(mono, sr, thresh_db=-50.0, min_gap=0.35, min_len=1.0):
    """Split on sustained silence."""
    win = int(0.02 * sr)
    n = len(mono)//win
    env = np.sqrt((mono[:n*win].reshape(n, win)**2).mean(axis=1) + 1e-12)
    db = 20*np.log10(env)
    loud = db > thresh_db
    segs, run, start = [], 0, None
    gapw = int(min_gap/0.02)
    for i, v in enumerate(loud):
        if v:
            if start is None: start = i
            run = 0
        else:
            if start is not None:
                run += 1
                if run >= gapw:
                    a, b = start*win, (i-run)*win
                    if (b-a)/sr >= min_len: segs.append((a, b))
                    start = None
    if start is not None:
        a, b = start*win, n*win
        if (b-a)/sr >= min_len: segs.append((a, b))
    return segs

TONES = {
    "Bell103 ans (2225)": 2225, "Bell103 orig mark (1270)": 1270,
    "ANS/ANSam (2100)": 2100, "V.25 ans (2100)": 2100,
    "guard (1800)": 1800, "V.22 carrier lo (1200)": 1200,
    "V.22 carrier hi (2400)": 2400, "V.32 carrier (1800)": 1800,
    "V.21ch2 mark (1650)": 1650, "V.21ch2 space (1850)": 1850,
    "V.8 CI/CM band": 1750,
}

def tone_profile(seg, sr):
    """Goertzel-ish magnitudes for the diagnostic tones."""
    out = {}
    N = len(seg)
    w = np.hanning(N)
    sp = np.abs(np.fft.rfft(seg*w))
    fr = np.fft.rfftfreq(N, 1/sr)
    tot = sp.sum() + 1e-12
    for name, f in TONES.items():
        m = (fr > f-25) & (fr < f+25)
        out[name] = float(sp[m].sum()/tot)
    return out, sp, fr

def main(path):
    x, sr = read_wav_raw(path)
    print(f"file        : {path}")
    print(f"sample rate : {sr} Hz")
    print(f"channels    : {x.shape[1]}")
    print(f"duration    : {len(x)/sr:.2f} s")
    if x.shape[1] == 2:
        l, r = x[:,0], x[:,1]
        diff = np.abs(l-r).max()
        cc = float(np.corrcoef(l, r)[0,1])
        print(f"L/R max|diff|: {diff:.6f}   correlation: {cc:.6f}")
        print(f"L rms {np.sqrt((l**2).mean()):.5f}   R rms {np.sqrt((r**2).mean()):.5f}")
        print("=> DUAL MONO (single line tap duplicated)" if diff < 1e-4
              else "=> TRUE STEREO (channels differ - possibly split directions)")
    mono = x.mean(axis=1)
    peak = np.abs(mono).max()
    print(f"peak        : {peak:.4f}  ({20*np.log10(peak+1e-12):.1f} dBFS)")
    segs = segments(mono, sr)
    print(f"\n{len(segs)} segments (>1.0 s, split on >0.35 s of <-50 dBFS):\n")
    print(f"{'#':>3} {'start':>9} {'end':>9} {'len':>8}   dominant tones")
    print("-"*88)
    for i,(a,b) in enumerate(segs):
        seg = mono[a:b]
        prof,_,_ = tone_profile(seg[:min(len(seg), sr*3)], sr)
        top = sorted(prof.items(), key=lambda kv:-kv[1])[:3]
        tones = ", ".join(f"{k}={v*100:.1f}%" for k,v in top if v > 0.01)
        print(f"{i:>3} {a/sr:>8.2f}s {b/sr:>8.2f}s {(b-a)/sr:>7.2f}s   {tones}")
    return x, sr, segs

if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv)>1 else "WAV/ALL Old Modem Sounds (300 baud to 56K).wav")
