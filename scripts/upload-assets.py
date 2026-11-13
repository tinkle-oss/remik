#!/usr/bin/env python3
"""Attach archives/checksums without replacing any existing release asset."""
import argparse
import hashlib
import json
import subprocess
import tempfile
from pathlib import Path


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tag")
    args = parser.parse_args()
    archives = sorted(Path("dist").glob("*.tar.gz"))
    assert len(archives) == 4, "expected four native archives"
    checksums = Path("dist/SHA256SUMS")
    checksums.write_text("".join(f"{digest(path)}  {path.name}\n" for path in archives))
    existing = json.loads(subprocess.check_output(["gh", "release", "view", args.tag, "--json", "assets"], text=True))["assets"]
    names = {asset["name"] for asset in existing}
    for path in [*archives, checksums]:
        if path.name in names:
            with tempfile.TemporaryDirectory() as directory:
                subprocess.run(["gh", "release", "download", args.tag, "--pattern", path.name, "--dir", directory], check=True)
                assert digest(Path(directory) / path.name) == digest(path), f"refusing to overwrite {path.name}"
        else:
            subprocess.run(["gh", "release", "upload", args.tag, str(path)], check=True)
