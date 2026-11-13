#!/usr/bin/env python3
"""Create binary RESP seeds without committing generated fuzz corpora."""
from pathlib import Path

corpus = Path(__file__).parent / "corpus" / "decode"
corpus.mkdir(parents=True, exist_ok=True)
for name, wire in {
    "ping": b"*1\r\n$4\r\nPING\r\n",
    "binary": b"*3\r\n$3\r\nSET\r\n$1\r\nk\r\n$3\r\na\x00b\r\n",
    "huge-bulk": b"*1\r\n$18446744073709551615\r\n",
    "huge-array": b"*18446744073709551615\r\n",
    "partial": b"*2\r\n$3\r\nGET\r\n$2\r\na",
    "pipeline": b"*1\r\n$4\r\nPING\r\n" * 2,
}.items():
    (corpus / name).write_bytes(wire)
