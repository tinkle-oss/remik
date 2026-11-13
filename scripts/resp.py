"""Small, bounded RESP2 client for tooling, not a production client."""
import socket
from typing import BinaryIO


class Client:
    def __init__(self, host, port):
        self.socket = socket.create_connection((host, int(port)), timeout=3)
        self.stream: BinaryIO = self.socket.makefile("rb")

    def close(self):
        self.stream.close()
        self.socket.close()

    def command(self, *args):
        args = [a if isinstance(a, bytes) else str(a).encode() for a in args]
        self.socket.sendall(b"*%d\r\n" % len(args) + b"".join(b"$%d\r\n" % len(a) + a + b"\r\n" for a in args))
        return self.reply()

    def reply(self, depth=0):
        if depth > 8:
            raise ValueError("reply nesting limit")
        line = self.stream.readline(4096)
        if not line.endswith(b"\r\n"):
            raise ValueError("incomplete RESP reply")
        kind, value = line[:1], line[1:-2]
        if kind in (b"+", b"-"):
            return kind, value
        if kind == b":":
            return int(value)
        size = int(value)
        if size == -1:
            return None
        if not 0 <= size <= 16 * 1024 * 1024:
            raise ValueError("reply size limit")
        if kind == b"$":
            data = self.stream.read(size + 2)
            if len(data) != size + 2 or not data.endswith(b"\r\n"):
                raise ValueError("incomplete bulk reply")
            return data[:-2]
        if kind == b"*" and size <= 1024:
            return [self.reply(depth + 1) for _ in range(size)]
        raise ValueError("unexpected RESP type")
