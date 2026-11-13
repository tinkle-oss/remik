#!/usr/bin/env python3
"""Collect production dependency license/notice texts. Run once per shipped target."""
import argparse
import json
import subprocess
from pathlib import Path


def production_ids(metadata):
    nodes = {n["id"]: n for n in metadata["resolve"]["nodes"]}
    seen = set()
    pending = [metadata["resolve"]["root"]]
    while pending:
        current = pending.pop()
        if current in seen:
            continue
        seen.add(current)
        for dep in nodes[current]["deps"]:
            if any(kind["kind"] in (None, "build") for kind in dep["dep_kinds"]):
                pending.append(dep["pkg"])
    return seen - {metadata["resolve"]["root"]}


def render(metadata):
    selected = production_ids(metadata)
    result = ["Remik third-party notices\nGenerated from Cargo.lock; production/build dependencies only.\n"]
    for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
        if package["id"] not in selected:
            continue
        root = Path(package["manifest_path"]).parent
        files = {p for p in root.rglob("*") if p.is_file() and p.name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE", "COPYRIGHT"))}
        if package.get("license_file"):
            files.add(root / package["license_file"])
        if not files:
            raise ValueError(f"{package['name']} has no license text; review before distribution")
        result.append(f"\n{'=' * 72}\n{package['name']} {package['version']}\nLicense: {package.get('license')}\nSource: {package.get('repository') or package.get('source')}\n")
        for path in sorted(files):
            if not path.resolve().is_relative_to(root.resolve()):
                raise ValueError(f"license path outside dependency: {path}")
            result.append(f"\n--- {path.relative_to(root)} ---\n{path.read_text(encoding='utf-8')}\n")
    return "".join(result)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target")
    parser.add_argument("--output", default="target/THIRD_PARTY_NOTICES.txt")
    args = parser.parse_args()
    target = args.target
    if not target:
        target = next(line.removeprefix("host: ") for line in subprocess.check_output(["rustc", "-vV"], text=True).splitlines() if line.startswith("host: "))
    data = json.loads(subprocess.check_output(["cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", target]))
    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(render(data))
