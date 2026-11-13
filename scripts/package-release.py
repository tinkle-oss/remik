#!/usr/bin/env python3
"""Build a deterministic archive layout; never publish from this script."""
import argparse
import gzip
import io
import subprocess
import tarfile
import tomllib
from pathlib import Path


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target")
    parser.add_argument("tag")
    args = parser.parse_args()
    version = tomllib.loads(Path("Cargo.toml").read_text())["package"]["version"]
    assert args.tag == "v" + version
    binary = Path("target/release/remik")
    assert subprocess.check_output([str(binary), "--version"], text=True).strip() == f"remik {version}"
    epoch = int(subprocess.check_output(["git", "show", "-s", "--format=%ct", "HEAD"], text=True))
    name = f"remik-{version}-{args.target}"
    Path("dist").mkdir(exist_ok=True)
    with open(f"dist/{name}.tar.gz", "wb") as output:
        with gzip.GzipFile(fileobj=output, mode="wb", filename="", mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode="w") as archive:
                for source, filename in [(binary, "remik"), (Path("LICENSE"), "LICENSE"), (Path("target/THIRD_PARTY_NOTICES.txt"), "THIRD_PARTY_NOTICES.txt")]:
                    data = source.read_bytes()
                    info = tarfile.TarInfo(f"{name}/{filename}")
                    info.size, info.mtime = len(data), epoch
                    info.mode = 0o755 if filename == "remik" else 0o644
                    archive.addfile(info, io.BytesIO(data))
