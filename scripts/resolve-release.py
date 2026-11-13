#!/usr/bin/env python3
"""Resolve an existing release tag to a commit on main; emit validated outputs."""
import argparse
import json
import os
import re
import subprocess


def gh(*args):
    return json.loads(subprocess.check_output(["gh", *args], text=True))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tag")
    parser.add_argument("--expected-sha")
    args = parser.parse_args()
    assert re.fullmatch(r"v(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-rc\.[1-9][0-9]*)?", args.tag), "invalid release tag"
    repo = os.environ["GITHUB_REPOSITORY"]
    release = gh("release", "view", args.tag, "--repo", repo, "--json", "tagName,isDraft,isPrerelease")
    assert release["tagName"] == args.tag and not release["isDraft"]
    assert release["isPrerelease"] == ("-rc." in args.tag), "GitHub prerelease state mismatch"
    obj = gh("api", f"repos/{repo}/git/ref/tags/{args.tag}")["object"]
    for _ in range(8):
        if obj["type"] == "commit":
            break
        assert obj["type"] == "tag", "tag must resolve to a commit"
        obj = gh("api", f"repos/{repo}/git/tags/{obj['sha']}")["object"]
    assert obj["type"] == "commit" and re.fullmatch(r"[0-9a-f]{40}", obj["sha"])
    sha = obj["sha"]
    if args.expected_sha:
        assert sha == args.expected_sha, "release-please SHA/tag mismatch"
    subprocess.run(["git", "merge-base", "--is-ancestor", sha, "origin/main"], check=True)
    with open(os.environ["GITHUB_OUTPUT"], "a") as output:
        output.write(f"tag={args.tag}\nsha={sha}\nversion={args.tag[1:]}\n")
