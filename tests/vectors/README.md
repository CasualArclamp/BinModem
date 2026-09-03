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
