# Releases

The source version is `0.1.0-rc.1`, an experimental prerelease. A version string in [`Cargo.toml`](../Cargo.toml) does not mean a GitHub release or GHCR image has been published. Publication requires administrator setup and successful workflow evidence; runtime credentials and remote repository settings were not available for verification during documentation work.

## Version and distribution policy

Remik uses [SemVer](https://semver.org/spec/v2.0.0.html) prerelease identifiers, with a `v` prefix on Git tags only. Pre-1.0 and candidate APIs are not stable. The automated policy accepts normal versions or `-rc.N` versions with positive candidate numbers; it does not promise every possible SemVer suffix.

[`release-please-config.json`](../release-please-config.json) uses the **Rust** release strategy, `versioning: prerelease`, `prerelease: true`, and `prerelease-type: rc`, with a single root package. [.release-please-manifest.json](../.release-please-manifest.json) starts as **`{}`**, because no initial release is recorded. Ordinary fixes/features after `0.1.0-rc.1` advance the candidate counter under this strategy; stable promotion is a separate reviewed action.

| Artifact | Intended publication contract |
| --- | --- |
| GitHub tag/release | `v0.1.0-rc.1` initially; marked as a GitHub prerelease |
| Native archives | Linux GNU amd64/arm64 and macOS Intel/Apple Silicon; native build and test per platform |
| GHCR images | Linux `amd64` and `arm64`; exact version `ghcr.io/tinkle-oss/remik:0.1.0-rc.1` and moving `:rc`, **only once published** |
| Disallowed RC aliases | No `:latest`, `:0`, or `:0.1` |
| crates.io | No publication; [`Cargo.toml`](../Cargo.toml) sets `publish = false` |

Use the local `remik:local` build in the [quick start](../README.md#run-with-docker) until artifacts have actually shipped and are accessible. There is no bootstrap edge image. A new GHCR package may initially be private; public visibility needs an intentional maintainer action.

## Administrator setup

The canonical setup and recovery runbook is [`.github/TOOLING.md`](../.github/TOOLING.md#release-setup-and-bootstrap-maintainer-action-required). The following summarizes its prerequisites, without claiming they are already configured:

1. Create and install a GitHub App on `tinkle-oss/remik` only, with repository **Contents, Pull requests, and Issues: read/write**. Set Actions variable `RELEASE_APP_ID` and Actions secret `RELEASE_APP_PRIVATE_KEY`. Never put the private key in the repository. The workflow mints a short-lived installation token with those explicit permissions.
2. Allow Actions to create PRs, protect `main`, and require the exact status **CI required** from [`.github/workflows/ci.yml`](../.github/workflows/ci.yml). Confirm the repository's Actions token can publish GHCR packages and has access to the package.
3. Keep the release manifest empty for bootstrap. The initial Conventional Commit on main must include this footer **once**:

   ```text
   Release-As: 0.1.0-rc.1
   ```

4. Let the official `googleapis/release-please-action` v4 create the release PR. Review its version changes and changelog, and merge only after required CI passes. It updates `Cargo.toml`, root `Cargo.lock`, fuzz lock package version via its TOML updater, release manifest, and `CHANGELOG.md`.

Do not seed the manifest with rc.1 as though it shipped, rely on an `initial-version` override for the Rust strategy, or leave a permanent `release-as` setting. There is deliberately no `GITHUB_TOKEN` fallback for Release Please: a bot-created PR from that token would not trigger the intended downstream CI. Without setup, the workflow emits a setup notice rather than publishing. This checkout does not establish that an App or its credentials exist.

## Publication flow

[`.github/workflows/release.yml`](../.github/workflows/release.yml) runs on trusted main, with planning, verification, native archives, asset upload, and image publication in one workflow. It does not depend on a tag-event cascade. The App token creates release PRs/releases; publication jobs use narrowly scoped `GITHUB_TOKEN` permissions (`contents: write` for release assets, `packages: write` for images).

[`scripts/resolve-release.py`](../scripts/resolve-release.py) resolves the existing release tag to an exact commit SHA, checks GitHub draft/prerelease state, and requires the commit to be in main's history. Subsequent jobs check out that SHA, not moving main. Full CI runs again against it. [`scripts/check-release.py`](../scripts/check-release.py) checks tag, Cargo version, both lockfiles, manifest, and release policy before publishing.

[`scripts/package-release.py`](../scripts/package-release.py) builds archives named `remik-<version>-<rust-target>.tar.gz`, each containing the binary, `LICENSE`, and `THIRD_PARTY_NOTICES.txt`. The four Rust targets are `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, and `aarch64-apple-darwin`. Linux binaries need compatible glibc; these are not static musl builds. [`scripts/upload-assets.py`](../scripts/upload-assets.py) attaches the four archives and `SHA256SUMS`.

Images carry license and platform-combined dependency notices, [OCI metadata](https://github.com/opencontainers/image-spec/blob/v1.1.1/annotations.md) for source/title/description/license/version/revision, and BuildKit SBOM plus max-mode provenance attestations. The workflow requests these for the multiarch image; it does not implement native-binary signing, a signed checksum manifest, or a Sigstore identity-verification policy. Do not equate an SBOM or checksum with authenticated provenance.

## Verify published artifacts

Start from the official [GitHub releases page](https://github.com/tinkle-oss/remik/releases) over HTTPS. Confirm the release tag, prerelease state, associated commit, and successful Release/CI runs before downloading. Download `SHA256SUMS` and the archive for your platform from the same release. With both in a directory, this stdlib check verifies each locally downloaded archive listed in the manifest:

```sh
python3 - <<'PY'
import hashlib
from pathlib import Path

checked = 0
for line in Path("SHA256SUMS").read_text().splitlines():
    expected, name = line.split(maxsplit=1)
    if Path(name).name != name:
        raise SystemExit("Unexpected path in checksum manifest")
    archive = Path(name)
    if not archive.is_file():
        continue  # You may have downloaded only your platform's archive.
    if hashlib.sha256(archive.read_bytes()).hexdigest() != expected:
        raise SystemExit(f"Checksum mismatch: {name}")
    print(f"OK: {name}")
    checked += 1
if not checked:
    raise SystemExit("No downloaded archives matched SHA256SUMS")
PY
```

A checksum detects corruption relative to the downloaded manifest; it does not independently authenticate the publisher. The current native-artifact trust boundary is GitHub's HTTPS release and repository/workflow access controls. No public signing key or verified signing identity is designated.

For an image **after publication**, inspect the exact tag using `docker buildx imagetools inspect ghcr.io/tinkle-oss/remik:0.1.0-rc.1`. Confirm both Linux platforms, inspect the attached SBOM/provenance and the platform images' OCI version/revision labels, and compare their source revision with the resolved release SHA. Record and deploy the resolved digest for reproducibility; `:rc` can move. Inspection alone is not cryptographic publisher-identity verification. Do not infer a signed-attestation guarantee from the presence of BuildKit metadata.

## Recovery and stable promotion

For partial failure, rerun failed jobs while successful planning outputs are retained, or manually dispatch **Release** on main with `existing_tag` set to the existing tag, such as `v0.1.0-rc.1`. This recovery path does not create a replacement release or tag. It repeats checks on the resolved tag SHA and refuses conflicting existing asset checksums. Existing image versions are skipped only when both platform version/revision labels match; authentication/network ambiguity fails closed. Recovery does not rewind the moving `:rc` alias.

Never automatically delete or replace a published tag/artifact to make a rerun pass. Investigate differing checksums and incomplete artifacts before manual intervention. Follow the [canonical recovery details](../.github/TOOLING.md#release-setup-and-bootstrap-maintainer-action-required), including package-API handling and alias behavior.

To intentionally promote stable, review a configuration change to `versioning: default` and `prerelease: false`, and use a new Conventional Commit footer `Release-As: 0.1.0`. Let Release Please produce the version PR. Stable releases currently receive **only the exact image version**, not automatic `latest`, major, or minor aliases. Support scope remains governed by [SECURITY.md](../SECURITY.md#support-scope); a tag named stable does not itself establish an operational SLA.
