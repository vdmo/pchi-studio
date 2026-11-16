"""A stand-in for TiXL's OscInput: listens on a UDP port and prints each OSC message PCHI sends.

    python3 osc_listen.py 9000

Use it to see exactly what PCHI sends before (or without) TiXL. It decodes address and int/float/string arguments only.
Stdlib only. Not TiXL: it proves what goes on the wire, not what TiXL does with it.
"""
import socket
import struct
import sys


def _pad(n):
    return (n + 4) & ~3


def decode(data):
    end = data.index(b"\0")
    addr = data[:end].decode()
    i = _pad(end)
    if i >= len(data) or data[i:i + 1] != b",":
        return addr, []
    tend = data.index(b"\0", i)
    tags, i, args = data[i + 1:tend].decode(), _pad(tend), []
    for t in tags:
        if t == "i":
            args.append(struct.unpack(">i", data[i:i + 4])[0]); i += 4
        elif t == "f":
            args.append(round(struct.unpack(">f", data[i:i + 4])[0], 6)); i += 4
        elif t == "s":
            e = data.index(b"\0", i); args.append(data[i:e].decode()); i = _pad(e)
        else:
            args.append(f"<{t}?>")
    return addr, args


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 9000
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.bind(("0.0.0.0", port))
    print(f"listening for OSC on UDP {port} ...", flush=True)
    while True:
        d, frm = s.recvfrom(4096)
        a, args = decode(d)
        print(f"{frm[0]}  {a}  {args}", flush=True)
