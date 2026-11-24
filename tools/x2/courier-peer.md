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
      --at "ATX1S32=66S54=0S58=48&A3&B1Q0DT5551234" --serial-input-after 5000000 `
      --summary > courier.json

`S32=66` leaves the Courier x2 and no V.90. Then this end:

    cargo run --release -p modem --example courier_peer -- 127.0.0.1:47123 [--pad 0|3|6] [--no-x2]

It prints what this modem says and does; the Courier's terminal is the
`serial_text` of `courier.json`. The emulator runs at about 1.2 times
real time here: 400 M instructions is some 118 s of line.

## What the emulator can and cannot show

libv90modem found (its `docs/history/x2-facts.md`, "Harness") that this
emulated Courier decodes x2 at the start-up's 32000 from any server, its
own I-modem's firmware included, but not above it after its rate change:
the same octets from three servers, the original among them, fail alike.
So a run here shows the negotiation, the training, records and data at
32000, and that the rate change is followed; only a real Courier can show
the rates above.
