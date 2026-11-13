# Build, CI, benchmarks, and release operations

Remik is a Rust in-memory key-value server. No live Redis instance
is needed by `cargo test`; only the Docker differential/benchmark tooling starts
a disposable, digest-pinned Redis reference. Nothing publishes to crates.io.

## Local checks and CI

Prerequisites: the `rust-toolchain.toml` toolchain (Rust **1.99.1**, rustfmt,
Clippy), Python **3.11+**, Docker Engine/Desktop with Compose v2 and Buildx,
ShellCheck, and actionlint **1.7.12**. Linux builds use the platform C linker;
macOS needs Xcode Command Line Tools. Rust MSRV **1.99.0** is tested separately
from the pinned development compiler; stable is a forward-compatibility test.
There is no native library/runtime or database installation requirement.

```sh
cargo fmt --all -- --check
cargo fmt --manifest-path fuzz/Cargo.toml -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
RUSTDOCFLAGS='-D warnings' cargo doc --locked --no-deps
cargo bench --locked --no-run
cargo bench --locked --bench core -- --test
python3 scripts/test-tooling.py
python3 scripts/check-docs.py
python3 scripts/check-release.py
actionlint .github/workflows/*.yml
shellcheck scripts/*.sh
docker compose config --quiet
docker compose -f compose.benchmark.yaml config --quiet
```

Install security tools deliberately (CI pins these exact versions):

```sh
cargo install cargo-deny --locked --version 0.20.2
cargo deny --locked check advisories licenses sources bans
cargo deny --manifest-path fuzz/Cargo.toml --locked check advisories licenses sources bans
# Install Trivy 0.76.2 using its verified upstream release package.
```

The aggregate check in `.github/workflows/ci.yml` is named **CI required**; the
name does not make it a GitHub-required merge gate. It fails if
any Rust matrix, tooling, dependency-policy, image scan, smoke, or differential
job fails, is canceled, or is skipped. No path filters hide checks. Maintainers
must manually review passing CI results before merging as project policy.
PR jobs have only `contents: read`, no secrets, no `pull_request_target`, and do
not publish images. Weekly CI refreshes security evidence even without commits.
Advisory/Trivy databases intentionally refresh: scanner versions and dependencies
are pinned, but vulnerability results can change. No blanket advisory ignores;
new license requirements require review. The NCSA exception is only for the
libFuzzer runtime in the separate unpublished fuzz workspace.

## Remote main policy

Exactly one active branch ruleset is already applied:
[27232283](https://github.com/tinkle-oss/remik/rules/27232283), targeting
`refs/heads/main`. Its only rule is `pull_request`, with zero required approvals,
no bypass actors (including administrators and bots), all optional review and
conversation settings off, and no required status checks. Classic branch
protection was removed (`branchProtectionRules.totalCount` is `0`). GitHub
requires a PR but does not enforce a CI merge gate; CI review is manual.

## Docker and Compose

First generate notices for the platform being distributed (the generation is
mandatory; Docker fails closed when the file is missing):

```sh
python3 scripts/notices.py --target x86_64-unknown-linux-gnu
docker build -t remik:ci .
python3 scripts/docker-smoke.py remik:ci
trivy image --scanners vuln --ignore-unfixed --severity HIGH,CRITICAL --exit-code 1 remik:ci
cargo build --locked --bin remik
python3 scripts/differential.py
docker compose up --build -d --wait
docker compose down
```

On an ARM host use `aarch64-unknown-linux-gnu` for image notices. A cross-platform
release combines both platforms' notices. The stdlib collector walks only
normal/build dependency edges, includes original license/copyright/notice texts,
and refuses dependencies without discoverable text. Review new dependency
licenses and upstream NOTICE obligations before distribution; this collector is
not a legal opinion. Native archives and images both carry LICENSE and these
notices. No source archives or Redis/memtier binaries are redistributed by it.

The image uses matching Debian bookworm builder/runtime, UID/GID **10001**, exec
entrypoint, and native platform compilation. Runtime has no compiler or package
installation step. `.dockerignore` is an allowlist: source, manifests, benches,
LICENSE, and the generated notices only; never `.agents`, `.git`, secrets, or
host binaries. Compose publishes only `127.0.0.1:6379`, uses a read-only root,
drops all capabilities, disallows privilege escalation, and limits CPU, RAM and
PIDs. There is no Remik data volume. The built-in healthcheck uses the same bind
and password-file environment as the server; wildcard binds probe loopback.
Smoke checks exercise both unauthenticated and password-file configurations and
require clean exit status 0 on SIGTERM while a client is still connected.

Remik is memory-only; stopping it loses all data. Its database byte budget is
not an RSS cap. No TLS/ACL is provided: do not expose this unauthenticated sample
configuration to untrusted networks. For AUTH, mount a readable regular secret
file at runtime and set `REMIK_PASSWORD_FILE`; never bake it into the image or
put its contents in CLI arguments. Host files must be readable by UID 10001.

## Fuzzing

`fuzz/` is its own Cargo workspace and has its own committed lockfile. Never
generate or update the root lockfile to install cargo-fuzz. To deliberately
refresh fuzz dependencies, run `cargo generate-lockfile --manifest-path
fuzz/Cargo.toml` and review that diff only. Release Please updates the `remik`
version in this lock using its TOML updater as well as the root Cargo files.
The pinned Release Please 17.3.0 parser wraps TOML scalar nodes: its extra-file
selector must match `@.name.value`, not `@.name`. The Python CI contract checks
this selector. To exercise the actual pinned updater without adding Node/npm
dependencies to every CI run, install it into a disposable directory **outside
the checkout**, then run the optional integration regression:

```sh
# Verify the chosen parent directory exists before creating this directory.
mkdir /path/to/external/remik-updater
npm install --prefix /path/to/external/remik-updater --ignore-scripts --no-audit --no-fund --save-exact release-please@17.3.0
node scripts/test-release-updater.cjs /path/to/external/remik-updater
```

It reproduces the legacy selector's no-op, bumps the next rc on copies of the
real root/fuzz lockfiles and Cargo manifest, checks that only Remik's fuzz-lock
version changes, and validates TOML/versions/manifest with the release checker.
All fixture output stays under that external prefix; checkout files stay intact.

Fuzzing needs a C++ compiler, LLVM/libFuzzer prerequisites and the **nightly-
2026-11-12** Rust toolchain (available upstream at implementation time):

```sh
cargo +nightly-2026-11-12 install cargo-fuzz --locked --version 0.13.2
cargo +nightly-2026-11-12 metadata --manifest-path fuzz/Cargo.toml --locked --format-version 1 > /dev/null
python3 fuzz/seed-corpus.py
cargo +nightly-2026-11-12 fuzz build decode
cargo +nightly-2026-11-12 fuzz run decode -- -max_total_time=300 -max_len=65536 -rss_limit_mb=512 -timeout=5
```

The scheduled/manual workflow builds before running. Arbitrary input is tested
with default and smaller limits; successful parses must consume bounded bytes
and return bounded arguments. ASan, RSS, input-length and per-input deadlines
bound the run; crashes/timeouts fail it. Corpus and crash artifacts are retained
14 days, including failures. PRs run the ordinary protocol/property tests, not a
five-minute fuzz campaign.

## Benchmarks

`scripts/benchmark.sh` needs coreutils `timeout` (`gtimeout` also accepted on
macOS). It runs from a checkout with Cargo.lock, LICENSE, benches and source
present. It creates an isolated Compose project and fresh output directory;
cleanup stops/removes its servers and disposable Redis anonymous image volumes.
Ports **16379/16380** must be free. Do not run a second benchmark concurrently.

```sh
cargo bench --locked --bench core
scripts/benchmark.sh
```

Each server gets **1 CPU / 256 MiB container RAM**, **64 MiB database budget**,
128 clients maximum; Redis persistence is disabled and Redis I/O threads are 1.
The dataset is 10,000 keys with 128-byte values, deliberately below both budgets.
Each of three trials starts a fresh server, preloads the entire sequential
keyspace, warms up for 3 seconds and measures 10 seconds. Remik/Redis order
alternates; only one server runs at a time. Both use exactly the same memtier
client settings: 2 threads, 8 clients per thread, pipeline 8, SET:GET 1:10,
deterministic distinct client seeds (no `--randomize`), and the same Docker
bridge path. The generator gets 2 CPUs / 256 MiB to reduce client bottlenecks.
These are equal quotas, not isolated physical cores; generator saturation and
hosted-runner noise still matter. No performance threshold blocks PRs.

Use `--protocol=redis` for the baseline RESP2 wire format. Remik also implements
the restricted `HELLO 2` handshake. `--retry-on-error --max-retries=0` exposes error counters
without hiding rejected requests behind retries. `check-memtier.py` requires
valid JSON, positive counts/throughput, zero errors/retries/unanswered requests,
zero GET misses after preload, and exactly 10,000 preload requests. A zero
memtier process exit is not sufficient. Preload uses an explicit request count:
memtier's `allkeys` counts max-minus-min, excluding one inclusive endpoint.
Raw JSON, logs, rendered quotas, host
metadata, summaries and Criterion output are retained for 30 days. Only the
Criterion `--test` smoke and benchmark compilation run on PRs.

Before Docker builds, the benchmark captures `git-status.txt` (including
untracked files, respecting Git ignores such as `.agents`), input-scoped
`git-worktree.diff` and `git-index.diff`, and `source-inputs.json`. The latter
records HEAD (null before the first commit), per-file SHA256/size/mode or symlink
target, and an aggregate input fingerprint. It covers all source/bench files,
manifests, Docker/Compose/toolchain settings, generated notices and benchmark
scripts, including untracked inputs; it does not scan `.agents` or host binaries.
Dirty and pre-initial-commit checkouts are allowed, never labeled as HEAD alone.
Fingerprints are verified immediately after building and again after the trials;
a mismatch writes `source-inputs-after.json` and fails the run. These endpoint
checks cannot detect an edit restored during the build: avoid concurrent edits
or use an isolated checkout for publication-quality measurements. Review patches
and host metadata before sharing artifacts.

## Release setup and bootstrap (maintainer action required)

No initial edge image is pushed, and no GitHub release exists yet. Release Please
uses the built-in **GITHUB_TOKEN**; no GitHub App, `RELEASE_APP_ID`, or
`RELEASE_APP_PRIVATE_KEY` is required. Keep the repository default token read-only
and the workflow default at `contents: read`; only the `plan` job receives
`contents: write`, `pull-requests: write`, and `issues: write`.

1. In repository **Settings > Actions > General > Workflow permissions**, enable
   **Allow GitHub Actions to create and approve pull requests**. The repository
   attempt returned HTTP **409** because organization policy blocks enabling it.
   The current CLI session lacks `admin:org`, so an organization owner may need
   to change organization Actions settings first. This blocker is not confirmed
   resolved; missing repository/organization permission must fail the actual
   release action, not produce a successful setup-only notice.
2. Ensure the repository Actions token can write GHCR packages and the package
   grants this repository Actions access. Publication jobs use the scoped
   **GITHUB_TOKEN** (`packages: write` for images, `contents: write` for assets),
   not a registry password or PAT. New GHCR packages may initially be private;
   a maintainer must intentionally choose package visibility.
3. Keep `.release-please-manifest.json` **{}** before the first real release.
   The initial conventional main commit must contain this footer, **once**:

   ```text
   Release-As: 0.1.0-rc.1
   ```

   Do not seed the manifest with rc.1 as if it had already shipped. Rust's
   `initialReleaseVersion()` returns 0.1.0, so `initial-version` is not a reliable
   bootstrap override. Do not leave a permanent `release-as` configuration.
4. Review the generated release PR and passing CI results before merging. It
   updates Cargo.toml, root/fuzz lock versions, manifest, and CHANGELOG. With the prerelease strategy,
   ordinary fixes/features after **0.1.0-rc.1** increment the rc counter.

GitHub's [workflow-trigger documentation](https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/trigger-a-workflow)
states that `GITHUB_TOKEN`-generated PR `opened`, `synchronize`, and `reopened`
events create CI runs in an **approval-required** state. A maintainer with write
access selects **Approve workflows to run** in the PR merge box, then reviews the
results before merging. Token-created tag/release events do not trigger new
workflows, so publication remains in the same workflow.

Release Please is the official **googleapis/release-please-action v4**, pinned
to **5c625bfb5d1ff62eadeeb3772007f7f66fdcf071** (verified via `gh api`). It runs
only on trusted main pushes or manual runs on main in `tinkle-oss/remik`.
Downstream verification, four native archives, and Docker publication are in the
**same workflow**, not a tag/release event cascade. Every downstream checkout
uses the resolved **exact release SHA**, not moving main.
The tag must match Cargo, both lockfiles, manifest, and GitHub prerelease state,
and point to a commit in main's history. Full CI must pass against that SHA within
the release workflow before publication, independent of the merge policy.

Native archives: Linux GNU amd64/arm64 and macOS Intel/Apple Silicon, each built
and tested on native runners, with LICENSE, third-party notices and SHA256SUMS.
GNU Linux binaries require a compatible glibc; they are not static musl builds.
Images build Linux amd64/arm64 under Buildx/QEMU with BuildKit **SBOM and max-mode
provenance**, OCI source/title/description/license/version/revision labels.

An RC publishes **only** `ghcr.io/tinkle-oss/remik:0.1.0-rc.1` (or its next
version) and moving **:rc**. Never `:latest`, `:0`, or `:0.1` for prereleases.
Stable releases also get **only their exact version**; stable aliases are not
automated. To intentionally promote stable: review a config change to
`versioning: default`, `prerelease: false`, and use a new commit footer
`Release-As: 0.1.0`. Let Release Please update the versions in its PR. A future
stable alias policy needs an explicit reviewed change, not metadata defaults.

For a partial failure, rerun failed jobs while successful plan outputs are
available, or manually dispatch **Release** on main with `existing_tag` set to
the existing GitHub release tag. This never creates a replacement tag/release.
All checks run again against that tag's SHA. Existing release assets are kept
only if their SHA256 matches; conflicting files fail rather than overwrite.
An existing image version is skipped only if both Linux platforms' OCI
version/revision match. The authenticated GitHub package API establishes tag
absence (an uncreated GHCR package can return an ambiguous registry 403);
other authentication/network errors fail closed. Recovery passes `--exact-only`
to the tag generator: even when an old exact image is missing, it publishes only
that version and never creates, advances or rewinds the moving :rc alias.
Do not rerun an old release to promote an
alias; never delete/replace immutable artifacts automatically. Review unexpected
binary checksum differences before any manual intervention.

## Pins and updates

Verified multi-platform Docker index pins:

| Purpose | Tag | SHA256 |
| --- | --- | --- |
| Builder | rust:1.99.1-slim-bookworm | f2c004e3d2d8b547e8cf7ffbc29205385f4e71d23e53e4e383065db5e9f822cd |
| Runtime | debian:bookworm-slim | 734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc |
| Reference | redis:8.10.3 | 1f88291b4c57bdf249f02eb085a8669889b2d2954e91d4de5118a5da6b78dd53 |
| Generator | redislabs/memtier_benchmark:2.6.1 | 41849040f6ea0b335c31ae532b1488ce47ea6e8c48a086d661f3d6953d3b599d |

`redis:8.10.3-bookworm` does not exist; the verified official 8.10.3 image is
based on trixie. It is only a reference, never a Remik runtime base.
All external Actions are full commit-SHA pinned, with the tracked major/tag in
comments. Release/CI builders pin Buildx **0.39.0**, BuildKit **0.35.0** at
`sha256:aa632b921ef07411d2cb77292cd34213d4f7b384001d6fceef1e1f25bf9752bb`,
and QEMU/binfmt at
`sha256:400a4873b838d1b89194d982c45e5fb3cda4593fbfd7e08a02e76b03b21166f0`.
The SBOM generator `docker/buildkit-syft-scanner:stable-1` is pinned to
`sha256:873a7458c3c8ac8e32c7b796c10e1600b07b963d2aa000a72a2d97aaaa2d6315`.
Dependabot checks Cargo (root + fuzz), Actions, Docker and Compose
weekly; patch/minor updates are grouped per ecosystem, majors remain separate.
No automerge or dependency-update secrets are configured. Keep compiler/image
updates coordinated with rust-toolchain/MSRV owners; verify image availability
with `docker buildx imagetools inspect` and action refs with `gh api` before
updating pins. Runtime dependencies are locked, never resolved afresh in CI.
