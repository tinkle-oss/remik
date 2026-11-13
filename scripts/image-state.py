#!/usr/bin/env python3
"""Check immutable image version before publishing (after registry login)."""
import argparse
import json
import os
import subprocess


def verify_existing(data, version, revision):
    assert {"linux/amd64", "linux/arm64"} <= data.keys(), "existing version is not the complete multi-platform image"
    for platform in ("linux/amd64", "linux/arm64"):
        labels = data[platform]["config"]["Labels"]
        assert labels["org.opencontainers.image.revision"] == revision, "version already exists at a different SHA"
        assert labels["org.opencontainers.image.version"] == version


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version")
    parser.add_argument("revision")
    args = parser.parse_args()
    image = f"ghcr.io/tinkle-oss/remik:{args.version}"
    # A never-created GHCR package returns an ambiguous registry 403, even for
    # valid credentials. Use the authenticated package API to establish absence.
    versions = subprocess.run(["gh", "api", "--paginate", "--slurp", "/orgs/tinkle-oss/packages/container/remik/versions?per_page=100"], text=True, capture_output=True)
    if versions.returncode:
        assert "HTTP 404" in versions.stderr, versions.stderr
        exists = False
    else:
        pages = json.loads(versions.stdout)
        exists = any(args.version in entry["metadata"]["container"]["tags"] for page in pages for entry in page)
    if exists:
        data = json.loads(subprocess.check_output(["docker", "buildx", "imagetools", "inspect", image, "--format", "{{json .Image}}"], text=True))
        verify_existing(data, args.version, args.revision)
        publish = "false"
    else:
        publish = "true"
    with open(os.environ["GITHUB_OUTPUT"], "a") as output:
        output.write(f"publish={publish}\n")
