#!/usr/bin/env python3
"""Validate the release contract locally; --tag also emits safe Docker tags."""
import argparse
import json
import re
import tomllib
from pathlib import Path

IMAGE = "ghcr.io/tinkle-oss/remik"
VERSION = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-rc\.([1-9][0-9]*))?")


def image_tags(version, exact_only=False):
    if not VERSION.fullmatch(version):
        raise ValueError(f"unsupported release version: {version!r}")
    return [f"{IMAGE}:{version}"] + ([f"{IMAGE}:rc"] if "-rc." in version and not exact_only else [])


def check(root, tag=None, exact_only=False):
    package = tomllib.loads((root / "Cargo.toml").read_text())["package"]
    config = json.loads((root / "release-please-config.json").read_text())
    manifest = json.loads((root / ".release-please-manifest.json").read_text())
    assert set(config["packages"]) == {"."}, "single root package required"
    policy = config["packages"]["."]
    assert policy["release-type"] == "rust"
    assert policy["include-component-in-tag"] is False
    assert policy["include-v-in-tag"] is True
    assert policy["bump-minor-pre-major"] is True
    assert "initial-version" not in policy and "release-as" not in policy
    fuzz_update = next(file for file in policy["extra-files"] if isinstance(file, dict) and file.get("path") == "fuzz/Cargo.lock")
    assert fuzz_update == {
        "type": "toml", "path": "fuzz/Cargo.lock",
        "jsonpath": "$.package[?(@.name.value == 'remik')].version",
    }, "release-please 17.3.0 wraps TOML scalars; match name.value"
    if policy["prerelease"]:
        assert policy["versioning"] == "prerelease" and policy["prerelease-type"] == "rc"
    else:
        assert policy["versioning"] == "default"
    assert manifest == {} or manifest == {".": package["version"]}, "manifest/Cargo version drift"
    if not manifest:
        assert package["version"] == "0.1.0-rc.1", "empty manifest is bootstrap-only"
    lock = tomllib.loads((root / "Cargo.lock").read_text())
    packages = [p for p in lock["package"] if p["name"] == "remik" and "source" not in p]
    assert len(packages) == 1 and packages[0]["version"] == package["version"], "Cargo.lock drift"
    fuzz_lock = tomllib.loads((root / "fuzz/Cargo.lock").read_text())
    fuzz_package = next(p for p in fuzz_lock["package"] if p["name"] == "remik")
    assert fuzz_package["version"] == package["version"], "fuzz lock version drift"
    tags = image_tags(package["version"], exact_only)
    if tag:
        assert manifest, "a release must have a populated manifest"
        assert tag == "v" + package["version"], "release tag/Cargo version mismatch"
        assert policy["prerelease"] == ("-rc." in package["version"]), "release policy/version mismatch"
    return tags


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag")
    parser.add_argument("--exact-only", action="store_true", help="Recovery: never emit mutable aliases")
    args = parser.parse_args()
    print("\n".join(check(Path(__file__).resolve().parent.parent, args.tag, args.exact_only)))
