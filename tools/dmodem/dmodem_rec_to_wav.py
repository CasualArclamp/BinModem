"""Turn dmodemd's own recording of a call into a WAV that plays.

DMODEM_REC=<base> has dmodemd write what it received to <base>.rx and what it
sent to <base>.tx, as raw 16-bit samples at 8000 Hz with no header. This puts
the two side by side in one file: left is what D-Modem heard, right is what it
sent, the same way round as BinModem's own captures.

    python dmodem_rec_to_wav.py <base> [out.wav]
"""
import array
import sys
import wave

base = sys.argv[1]
out = sys.argv[2] if len(sys.argv) > 2 else base + ".wav"

rx = array.array("h")
tx = array.array("h")
with open(base + ".rx", "rb") as f:
    data = f.read()
    rx.frombytes(data[: len(data) & ~1])
with open(base + ".tx", "rb") as f:
    data = f.read()
    tx.frombytes(data[: len(data) & ~1])

n = min(len(rx), len(tx))
both = array.array("h", bytes(4 * n))
both[0::2] = rx[:n]
both[1::2] = tx[:n]

with wave.open(out, "wb") as w:
    w.setnchannels(2)
    w.setsampwidth(2)
    w.setframerate(8000)
    w.writeframes(both.tobytes())
print(f"{n / 8000:.1f} s as {out}")
