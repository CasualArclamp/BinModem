"""The terminal on D-Modem's end of the call.

dmodemd on Windows gives its DTE as a TCP port. This connects to it, sets the
modem to answer on the first ring in the modulation asked for, and prints what
the modem says from then on. A line with "hello" in it is answered, so that
there is something to see cross the call in both directions.

    python dmodem_dte.py [port] [modulation] [log file]
"""
import socket
import sys
import time

port = int(sys.argv[1]) if len(sys.argv) > 1 else 2323
ms = sys.argv[2] if len(sys.argv) > 2 else "V90"
log = open(sys.argv[3], "w", encoding="utf-8") if len(sys.argv) > 3 else None


def say(text):
    print(text, flush=True)
    if log:
        log.write(text + "\n")
        log.flush()


for _ in range(100):
    try:
        s = socket.create_connection(("127.0.0.1", port), timeout=1)
        break
    except OSError:
        time.sleep(0.2)
else:
    sys.exit(f"nothing is listening on 127.0.0.1:{port}; is dmodemd running?")

s.settimeout(0.5)
start = time.time()


def show(data):
    text = data.decode("latin-1").replace("\r", "")
    for line in text.split("\n"):
        if line.strip():
            say(f"[{time.time() - start:7.2f}] {line}")


for line in ("ATE0", "ATS0=1", f"AT+MS={ms}"):
    say(f"[{time.time() - start:7.2f}] > {line}")
    s.sendall(line.encode() + b"\r")
    time.sleep(0.5)

say("waiting for a call; Ctrl-C to stop")
while True:
    try:
        data = s.recv(4096)
    except socket.timeout:
        continue
    except (OSError, KeyboardInterrupt):
        break
    if not data:
        break
    show(data)
    if b"hello" in data and b"dmodem" not in data:
        s.sendall(b"hello from dmodem\r\n")
