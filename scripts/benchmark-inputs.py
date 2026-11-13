#!/usr/bin/env python3
"""Record dirty-checkout provenance; fail if build/benchmark inputs change."""
import argparse
import hashlib
import json
import stat
import subprocess
from pathlib import Path

INPUT_FILES = (
    "Dockerfile", ".dockerignore", "Cargo.toml", "Cargo.lock", "LICENSE",
    "rust-toolchain.toml", "compose.benchmark.yaml", "target/THIRD_PARTY_NOTICES.txt",
)
INPUT_DIRS = ("src", "benches", "scripts")


def fingerprint(root):
    paths = {root / name for name in INPUT_FILES}
    for directory in INPUT_DIRS:
        paths.update((root / directory).rglob("*"))
    files = {}
    for path in sorted(paths):
        if "__pycache__" in path.parts or path.suffix == ".pyc":
            continue
        name = path.relative_to(root).as_posix()
        if path.is_symlink():
            files[name] = {"symlink": str(path.readlink())}
        elif path.is_file():
            data = path.read_bytes()
            files[name] = {"sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data), "mode": stat.S_IMODE(path.stat().st_mode)}
    encoded = json.dumps(files, sort_keys=True, separators=(",", ":")).encode()
    return {"input_sha256": hashlib.sha256(encoded).hexdigest(), "files": files}


def capture(root, output):
    inputs = fingerprint(root)
    head = subprocess.run(["git", "rev-parse", "--verify", "HEAD"], cwd=root, text=True, capture_output=True)
    inputs["head"] = head.stdout.strip() if head.returncode == 0 else None
    status = subprocess.check_output(["git", "status", "--porcelain=v1", "--untracked-files=all"], cwd=root)
    (output / "git-status.txt").write_bytes(status)
    # Scope patches to build/tooling inputs: do not collect unrelated local files.
    for cached, filename in [(False, "git-worktree.diff"), (True, "git-index.diff")]:
        args = ["git", "diff", "--binary", "--no-ext-diff", "--no-textconv"]
        if cached:
            args.append("--cached")
        diff = subprocess.check_output([*args, "--", *INPUT_FILES, *INPUT_DIRS], cwd=root)
        (output / filename).write_bytes(diff)
    (output / "source-inputs.json").write_text(json.dumps(inputs, indent=2, sort_keys=True) + "\n")
    return inputs["input_sha256"]


def verify(root, output):
    before = json.loads((output / "source-inputs.json").read_text())
    after = fingerprint(root)
    (output / "source-inputs-after.json").write_text(json.dumps(after, indent=2, sort_keys=True) + "\n")
    # shortcut: endpoint hashes cannot detect edit-and-restore during a build;
    # use a quiescent/isolated checkout for publication-quality measurements.
    if before["input_sha256"] != after["input_sha256"]:
        raise ValueError("build/benchmark inputs changed; discard this run and retry (see source-inputs*.json)")
    return after["input_sha256"]


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("capture", "verify"))
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    print((capture if args.action == "capture" else verify)(root, args.output))
