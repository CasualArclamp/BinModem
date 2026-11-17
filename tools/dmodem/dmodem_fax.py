"""D-Modem as a receiving fax: class 2.0 over dmodemd's TCP DTE.

Answers the first ring, takes each page with AT+FDR and writes its image
data (T.4, or whatever was agreed) to <out-base>.pageN.t4. FIS is the
+FIS string: 1,5,0,2,0,0,0,0 for group 3 up to 14400, 1,13,0,2,0,2,0,0 to
allow V.34 with error correction.

   python fax_rx.py <port> <FIS> <out-base> <seconds>"""
import socket, sys, time
port, fis, base, secs = int(sys.argv[1]), sys.argv[2], sys.argv[3], float(sys.argv[4])
for _ in range(50):
    try: s = socket.create_connection(("127.0.0.1", port), timeout=1); break
    except OSError: time.sleep(0.2)
s.settimeout(0.2); t0 = time.time(); buf = b""
def log(m): print("[%7.2f] %s" % (time.time() - t0, m), flush=True)
def read():
    global buf
    try:
        b = s.recv(65536)
        if b: buf += b
    except socket.timeout: pass
def cmd(c, wait=1.0):
    global buf
    log("> " + c); s.sendall(c.encode() + b"\r"); end = time.time() + wait
    while time.time() < end and b"OK" not in buf and b"ERROR" not in buf: read()
    log("< %r" % buf); buf = b""
for c in ("ATE0", "AT+FCLASS=2.0", 'AT+FLI="DMODEM"', "AT+FCR=1", "AT+FIS=" + fis, "AT+FIS?"): cmd(c)
end = time.time() + secs; state = "ring"; page = 0; data = b""
while time.time() < end:
    read()
    if state == "ring" and b"RING" in buf:
        log("< RING"); buf = b""; log("> ATA"); s.sendall(b"ATA\r"); state = "neg"
    elif state == "neg" and (b"OK" in buf or b"+FHS" in buf or b"NO CARRIER" in buf or b"ERROR" in buf):
        log("< %r" % buf); done = b"+FHS" in buf or b"NO CARRIER" in buf or b"ERROR" in buf; buf = b""
        if done: break
        log("> AT+FDR"); s.sendall(b"AT+FDR\r"); state = "fdr"
    elif state == "fdr" and (b"CONNECT" in buf or b"+FHS" in buf or b"NO CARRIER" in buf or b"ERROR" in buf):
        i = buf.find(b"CONNECT")
        if i < 0: log("< %r" % buf); break
        log("< %r" % buf[:i + 7]); buf = buf[buf.find(b"\n", i) + 1:]
        s.sendall(b"\x12"); state = "page"; data = b""
    elif state == "page":
        data += buf; buf = b""
        j = 0; out = bytearray(); endat = -1
        while j < len(data) - 1:
            if data[j] == 0x10:
                if data[j + 1] == 0x03: endat = j; break
                if data[j + 1] == 0x10: out.append(0x10)
                j += 2
            else: out.append(data[j]); j += 1
        if endat >= 0:
            page += 1; open("%s.page%d.t4" % (base, page), "wb").write(bytes(out))
            log("page %d: %d octets of image data" % (page, len(out)))
            buf = data[endat + 2:]; state = "neg"
log("< %r" % buf); log("pages: %d" % page)
