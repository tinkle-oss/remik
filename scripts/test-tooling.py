#!/usr/bin/env python3
"""Regression checks for tooling trust boundaries (stdlib only)."""
import copy
import importlib.util
import io
import json
import tempfile
import unittest
from unittest import mock
from pathlib import Path

from notices import production_ids
from resp import Client


def load(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(name + ".py"))
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class ToolingTests(unittest.TestCase):
    def test_documentation_targets(self):
        check = load("check-docs").broken_links
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "README.md").write_text("[ok](Cargo.toml) [external](https://example.org/) [bad](missing.md)")
            (root / "Cargo.toml").write_text("")
            self.assertEqual(check(root), ["README.md: missing.md"])

    def test_release_tags(self):
        tags = load("check-release").image_tags
        self.assertEqual(tags("0.1.0-rc.1"), ["ghcr.io/tinkle-oss/remik:0.1.0-rc.1", "ghcr.io/tinkle-oss/remik:rc"])
        self.assertEqual(tags("1.2.3"), ["ghcr.io/tinkle-oss/remik:1.2.3"])
        self.assertEqual(tags("0.1.0-rc.1", exact_only=True), ["ghcr.io/tinkle-oss/remik:0.1.0-rc.1"])
        self.assertEqual(tags("1.2.3", exact_only=True), ["ghcr.io/tinkle-oss/remik:1.2.3"])
        for version in ("v1.2.3", "1.2.3;echo", "01.2.3", "1.2.3-rc.0", "1.2.3-beta.1"):
            with self.assertRaises(ValueError):
                tags(version)

    def test_release_contract_and_lock_drift(self):
        check = load("check-release").check
        config = json.loads((Path(__file__).parent.parent / "release-please-config.json").read_text())
        config["packages"]["."].update({"prerelease": True, "versioning": "prerelease", "prerelease-type": "rc"})
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "fuzz").mkdir()
            (root / "Cargo.toml").write_text('[package]\nname = "remik"\nversion = "0.1.0-rc.1"\n')
            lock = 'version = 4\n[[package]]\nname = "remik"\nversion = "0.1.0-rc.1"\n'
            (root / "Cargo.lock").write_text(lock)
            (root / "fuzz/Cargo.lock").write_text(lock)
            (root / "release-please-config.json").write_text(json.dumps(config))
            manifest = root / ".release-please-manifest.json"
            manifest.write_text("{}")
            check(root)
            with self.assertRaises(AssertionError):
                check(root, "v0.1.0-rc.1")
            manifest.write_text(json.dumps({".": "0.1.0-rc.1"}))
            check(root, "v0.1.0-rc.1")
            self.assertEqual(check(root, "v0.1.0-rc.1", exact_only=True), ["ghcr.io/tinkle-oss/remik:0.1.0-rc.1"])
            bad_config = copy.deepcopy(config)
            bad_config["packages"]["."]["extra-files"][0]["jsonpath"] = "$.package[?(@.name == 'remik')].version"
            (root / "release-please-config.json").write_text(json.dumps(bad_config))
            with self.assertRaisesRegex(AssertionError, "name.value"):
                check(root)
            (root / "release-please-config.json").write_text(json.dumps(config))
            with self.assertRaises(AssertionError):
                check(root, "v0.1.0-rc.2")
            (root / "fuzz/Cargo.lock").write_text(lock.replace("rc.1", "rc.2"))
            with self.assertRaises(AssertionError):
                check(root)

    def test_workflow_toolchain_and_recovery_overrides(self):
        root = Path(__file__).parent.parent
        ci = (root / ".github/workflows/ci.yml").read_text()
        self.assertIn("      RUSTUP_TOOLCHAIN: ${{ matrix.toolchain }}", ci)
        fuzz = (root / ".github/workflows/fuzz.yml").read_text()
        self.assertIn("    env:\n      RUSTUP_TOOLCHAIN: nightly-2026-11-12", fuzz)
        self.assertIn("          toolchain: nightly-2026-11-12", fuzz)
        release = (root / ".github/workflows/release.yml").read_text()
        self.assertIn("RECOVERY: ${{ inputs.existing_tag != '' }}", release)
        self.assertIn('if [[ "$RECOVERY" == \'true\' ]]; then tag_args+=(--exact-only); fi', release)
        self.assertIn('python3 scripts/check-release.py --tag "$TAG" "${tag_args[@]}"', release)

    def test_benchmark_dirty_input_fingerprints_and_race_guard(self):
        inputs = load("benchmark-inputs")
        script = (Path(__file__).parent / "benchmark.sh").read_text()
        self.assertLess(script.index('benchmark-inputs.py capture "$results"'), script.index('"${compose[@]}" build remik'))
        self.assertLess(script.index('"${compose[@]}" build remik'), script.index('benchmark-inputs.py verify "$results"'))
        self.assertEqual(script.count('benchmark-inputs.py verify "$results"'), 2)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "src").mkdir()
            source = root / "src/main.rs"
            source.write_text("fn main() {}\n")
            before = inputs.fingerprint(root)
            self.assertIn("src/main.rs", before["files"])
            # This source is untracked: fingerprints must not depend on git ls-files.
            source.write_text('fn main() { println!("changed"); }\n')
            changed = inputs.fingerprint(root)
            self.assertNotEqual(before["input_sha256"], changed["input_sha256"])
            (root / ".agents").mkdir()
            (root / ".agents/private").write_text("not a build input")
            (root / "target").mkdir()
            (root / "target/remik").write_bytes(b"unrelated binary")
            self.assertEqual(inputs.fingerprint(root), changed)
            (root / "source-inputs.json").write_text(json.dumps(before))
            with self.assertRaisesRegex(ValueError, "inputs changed"):
                inputs.verify(root, root)
            self.assertTrue((root / "source-inputs-after.json").is_file())
            # An unborn HEAD is supported; status includes untracked, not ignored files.
            with mock.patch.object(inputs.subprocess, "run", return_value=mock.Mock(returncode=1)), mock.patch.object(inputs.subprocess, "check_output", side_effect=[b"?? src/main.rs\n", b"", b""]):
                inputs.capture(root, root)
            self.assertIsNone(json.loads((root / "source-inputs.json").read_text())["head"])
            self.assertEqual((root / "git-status.txt").read_text(), "?? src/main.rs\n")
            inputs.verify(root, root)

    def test_memtier_errors_and_empty_results_fail(self):
        validate = load("check-memtier").validate
        totals = {"Errors": 0, "Connection Errors": 0, "Retry Attempts": 0, "Retried Ops": 0, "Count": 10000, "Ops/sec": 500, "Average Latency": 1, "Misses/sec": 0, "Percentile Latencies": {}}
        data = {"ALL STATS": {"Totals": totals, "Runtime": {"Unanswered requests": 0, "Interrupted": "false"}}}
        validate(data, 10000)
        for key in ("Errors", "Connection Errors", "Retry Attempts", "Retried Ops", "Misses/sec"):
            broken = copy.deepcopy(data)
            broken["ALL STATS"]["Totals"][key] = 1
            with self.assertRaises(AssertionError):
                validate(broken)
        for key in ("Errors", "Count"):
            broken = copy.deepcopy(data)
            del broken["ALL STATS"]["Totals"][key]
            with self.assertRaises(KeyError):
                validate(broken)
        for count in (0, 9999):
            broken = copy.deepcopy(data)
            broken["ALL STATS"]["Totals"]["Count"] = count
            with self.assertRaises(AssertionError):
                validate(broken, 10000)

    def test_notices_exclude_dev_dependencies(self):
        metadata = {"resolve": {"root": "root", "nodes": [
            {"id": "root", "deps": [{"pkg": "prod", "dep_kinds": [{"kind": None}]}, {"pkg": "dev", "dep_kinds": [{"kind": "dev"}]}]},
            {"id": "prod", "deps": [{"pkg": "build", "dep_kinds": [{"kind": "build"}]}]},
            {"id": "build", "deps": []}, {"id": "dev", "deps": []},
        ]}}
        self.assertEqual(production_ids(metadata), {"prod", "build"})

    def test_existing_images_must_match_both_platforms_and_revision(self):
        verify = load("image-state").verify_existing
        labels = {"org.opencontainers.image.version": "0.1.0-rc.1", "org.opencontainers.image.revision": "a" * 40}
        image = {platform: {"config": {"Labels": labels.copy()}} for platform in ("linux/amd64", "linux/arm64")}
        verify(image, "0.1.0-rc.1", "a" * 40)
        with self.assertRaises(AssertionError):
            verify(image, "0.1.0-rc.1", "b" * 40)
        with self.assertRaises(AssertionError):
            verify(image, "0.1.0-rc.2", "a" * 40)
        del image["linux/arm64"]
        with self.assertRaises(AssertionError):
            verify(image, "0.1.0-rc.1", "a" * 40)

    def test_resp_binary_null_and_truncation(self):
        client = Client.__new__(Client)
        client.stream = io.BytesIO(b"*3\r\n$3\r\na\x00b\r\n$-1\r\n:7\r\n")
        self.assertEqual(client.reply(), [b"a\x00b", None, 7])
        for wire in (b"", b"$3\r\na", b"$999999999999\r\n"):
            client.stream = io.BytesIO(wire)
            with self.assertRaises(ValueError):
                client.reply()


if __name__ == "__main__":
    unittest.main()
