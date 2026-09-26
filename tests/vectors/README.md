# Golden test vectors

Cut from `WAV/ALL Old Modem Sounds (300 baud to 56K).wav` — a direct line capture of a ~2005 Conexant V.92
softmodem forced to each modulation with `AT+MS`. Both directions are
summed on one tap, as on a real 2-wire line.

Resampled 44100 Hz stereo -> 16000 Hz mono.

| file | source window | duration | contents |
|---|---|---|---|
| `bell103-300.wav` | 3.80–16.60 s | 12.80 s | Bell 103 300 bps FSK; carries the login session |
| `v22bis-2400.wav` | 17.90–34.80 s | 16.90 s | ITU-T V.22bis 2400 bps, FDM full duplex |
| `v32bis-14400.wav` | 37.30–55.40 s | 18.10 s | ITU-T V.32bis 14400 bps, echo-cancelled |
| `v34-33600.wav` | 57.50–75.40 s | 17.90 s | ITU-T V.34 33600 bps, V.8 + line probing |
| `v90-56k.wav` | 77.70–102.40 s | 24.70 s | ITU-T V.90 56k, V.34-style startup |
| `v92-56k.wav` | 103.20–126.50 s | 23.30 s | ITU-T V.92 56k, V.34-style startup |

## fax-v34-cm.wav

Four seconds of a real V.34 fax (Super G3) calling BinModem on 2026-09-26, cut
from 6.0 s of a screen recording of the call: what the window played to the
speakers, so the far end only, through the softphone and a lossy codec.
48000 Hz stereo -> 16000 Hz mono.

It is V.21's low channel carrying one V.8 call menu over and over,
`E0 81 85 D4`: transmit facsimile from the call terminal; V.34 half-duplex,
V.17, V.29 half-duplex and V.27 ter. BinModem had answered with T.30's plain
2100 Hz tone, which the caller took for ANSam, and then sent its DIS straight
past a caller waiting for a JM.
