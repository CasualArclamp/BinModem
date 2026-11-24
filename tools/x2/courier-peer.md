# An emulated U.S. Robotics Courier as x2's test peer

courier-emu (github.com/cryan209/courier-emu) runs U.S. Robotics' own
Courier firmware in an emulated 80186 and TMS320C51. It has no licence, so
it is used as a black box on this machine only: nothing of it, and no ROM
image, goes into this repository. The Courier dials over its line link,
and `crates/modem/examples/courier_peer.rs` answers as the exchange and as
BinModem's V.90 server with x2 on offer (S70=1).

## Setting it up (outside the repository)

    git clone https://github.com/cryan209/courier-emu ../courier-emu-ref
    cd ../courier-emu-ref
    py -3 -m venv .venv
    .venv\Scripts\python -m pip install -e ".[execute,disasm]"

Its `./courier` launcher is a POSIX script; on Windows run
`.venv\Scripts\python.exe -m courier_emu` instead. Three things need local
changes on Windows (kept uncommitted in that checkout, marked
`LOCAL WINDOWS PATCH`):

- `courier_emu/line.py`: Windows Python has no `AF_UNIX`, so `--line-link
  tcp:HOST:PORT` uses loopback TCP with the same byte stream.
- `courier_emu/poll_state.py`: no `time.clock_gettime_ns`; falls back to
  `time.monotonic_ns()`.
- `native/c5x_capi.cpp`: winsock in place of `poll.h` and `sys/socket.h`.

MSVC cannot build the native cores (GCC builtins); MSYS2's MinGW g++ can.
From Git Bash in the checkout, with `PATH=/c/msys64/mingw64/bin:$PATH` and
`PY` the Python folder:

    g++ -std=c++17 -O3 -shared -fno-semantic-interposition native/c5x_core.cpp native/c5x_capi.cpp -o .build/libcourier_c5x.so -static -static-libgcc -static-libstdc++ -lws2_32
    g++ -std=c++17 -O3 -shared -DMS_WIN64 -I $PY/include native/x86_interpreter.cpp $PY/python314.dll -o .build/_x86_native.cp314-win_amd64.pyd -static-libgcc -static-libstdc++ -Wl,-Bstatic -lstdc++ -lwinpthread

## A run

The Courier (403 board image: supervisor 7.4.16, DSP 3.1.2, with x2),
listening on a TCP line, dialling once it has booted (PowerShell, in the
checkout):

    $env:COURIER_LINE_DIGITAL="1"; $env:COURIER_LINE_FRAME_MS="20"
    .venv\Scripts\python.exe -m courier_emu run dumps\courier-board-21210-capture-403\courier-board.rom `
      --instructions 400000000 --line-link tcp:127.0.0.1:47123 --line-listen --with-dsp `
      --nvram-fixture idsdl403 --board-id 7 --dip-preset default --tick-ms 5 `
      --at "ATX1S32=66S39=8S54=0S58=48&A3&B1Q0DT5551234" --serial-input-after 5000000 `
      --at "ATI11" --serial-input-after 150000000 `
      --summary > courier.json

`S32=66` leaves the Courier x2 and no V.90. `S39=8` matters: the profile's
own S39=0 is an invalid transmit level, and with it the Courier's upstream
arrives with a bad frame every few hundred octets (courier-emu's own
finding). The second `--at` types ATI11 once the call is over, for the
Courier's diagnostics. Then this end:

    cargo run --release -p modem --example courier_peer -- 127.0.0.1:47123 [--pad 0|3|6] [--no-x2] [--ms V90|V34] [--drive D]

Waveforms go to the line at modem-sip's drive, 0.1 unless `--drive` says
otherwise, and a digital modem's exact levels as they are, as modem-sip and
the window send them. It prints what this modem says and does, and the
Courier's hook and call state as they change; the Courier's terminal is the
`serial_text` of `courier.json`. The emulator runs at about 1.2 times
real time here: 400 M instructions is some 118 s of line.

## What it showed (2026-11-24, courier-emu e84e324)

- The Courier takes x2: ATI11 says `x2/V.34+`, `8000/3200`, 64 states,
  non-linear encoding and shaping upstream. Its start-up MP is the
  published `0344 03fe 0000 0500`.
- `CONNECT 49333/ARQ/x2/LAPM/V42BIS` on its terminal; this end reports
  32000 down and 24000 up (USR prints one 1333 step above the payload).
- 12 s in, its rate change asks for index 14: followed to 54666 down and
  28800 up in 0.7 s, and every one of the 82 numbered lines sent in the
  call, some 70 of them after the change, arrived intact.

libv90modem had found (its `docs/history/x2-facts.md`, "Harness") that
this emulator would not carry data above 32000 from any server. courier-emu
has since changed how the Courier DSP's frame interrupt is clocked (its
`docs/courier-x2-frame-default-20261031.md`), and that limit is gone here.

Two things this run found, both fixed:

- The Courier's V.8 classifier looks for the server's carrier turned half
  way round between one JM and the next, on the bit boundary where the
  next JM starts. One bit early, at the start of the last stop bit, it
  does not count, and the Courier calls the server "not x2". Watching its
  DSP cells `039f` (the classifier's verdict, `4040` when taken) and
  `fff7` (the x2 conditions, bit 4 "remote modem supports x2") with
  `--dsp-write-watch` is how that was found.
- V.42 here answered the Courier's XID with its own offer (N7 250 against
  its 32, 32-bit FCS and multiple SREJ it had not offered); the Courier
  repeated its XID and never sent SABME.

## Plain V.34 against it (2026-11-24)

With `--no-x2` (or `--ms V34`) the Courier and this modem run V.34. It
failed in phase 3 at first, for two reasons:

- This harness sent the modem's output undriven. Every transmitter here
  leaves at 0.707 rms and modem-sip and the window drive the line at a
  tenth of that; without it V.34's phase 2 and 3 reached the Courier at
  about +3 dBm0, 18 to 37 % of the samples flat against the top of
  mu-law, and the Courier never trained on our PP and TRN: it sent no S,
  and this end gave up "listening for S". V.8 had been at -5.8 dBm0 and
  went through.
- Driven properly, this end chose 3429 baud towards the Courier (its
  INFO1c projects 9, 8, 7, 6, 5, 4 for 3429 down to 2400 in every run here,
  clipped or not). Its MP then asked for 7200 at 3429, it never took
  our MP, and it retrained; every retrain chose 3429 again. The Courier's
  codec runs each symbol rate at its own sample rate, 8000 Hz for 3429,
  with the least margin of the six (courier-emu
  `docs/codec-sample-rates.md`); whether the real board would do better is
  not known. BinModem now keeps off, for the rest of the call, a symbol
  rate a start-up reached phase 4 at and then retrained from before data
  mode. libv90modem never offers
  3429, which is why it got 3200 straight away.

The run: the first start-up at 3429 retrains, the second at 3200 gives
`CONNECT 31200/ARQ/V34/LAPM/V42BIS` on the Courier's terminal and 31200
both ways here. Text crossed both ways, and every numbered line that
left before the line closed (79 of 80) arrived intact.
