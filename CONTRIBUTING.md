# Contributing to Remik

Remik accepts bug reports, documentation improvements, tests, and scoped code changes through [GitHub issues](https://github.com/tinkle-oss/remik/issues) and pull requests. Read the [code of conduct](CODE_OF_CONDUCT.md) and [implemented compatibility](docs/compatibility.md) before proposing behavior changes.

## Discuss the change

Search existing issues first, then choose the matching form:

- [Bug report](https://github.com/tinkle-oss/remik/issues/new?template=bug_report.yml): include the version, environment, minimal reproduction, and separate expected and actual results.
- [Feature request](https://github.com/tinkle-oss/remik/issues/new?template=proposal.yml): describe the problem and proposed behavior; keep small requests brief.
- [Usage question](https://github.com/tinkle-oss/remik/issues/new?template=question.yml): ask about configuration or documented behavior without proposing an API.

Keep public examples and diagnostics redacted; never include passwords, secret-file contents, private keys, or private data values. Send vulnerabilities through [private reporting](https://github.com/tinkle-oss/remik/security/advisories/new); see [the security policy](SECURITY.md#report-a-vulnerability) for details.

Propose major changes in an issue before implementation. Explain the use case, observable contract, compatibility impact, failure/resource behavior, alternatives, and how it will be tested. New command families, persistence, replication, authentication changes, and concurrency redesigns need this discussion. The [roadmap](docs/roadmap.md) contains evaluation topics, not preapproved features.

## Set up a checkout

Use Linux or macOS with a native C linker; macOS needs Xcode Command Line Tools. [`rust-toolchain.toml`](rust-toolchain.toml) pins Rust `1.99.1`, rustfmt, and Clippy. [`Cargo.toml`](Cargo.toml) declares MSRV `1.99`; CI tests `1.99.0` separately, plus stable for forward compatibility.

```sh
rustup toolchain list
# If missing, install the project toolchain without changing your global default.
rustup toolchain install 1.99.1 --profile minimal --component rustfmt --component clippy
cargo --version
cargo test --locked
```

Run Cargo from the repository so it selects the project pin. Do not change your global toolchain, lower the pin/MSRV, or regenerate lockfiles merely to work around unavailable tooling. If an exact toolchain or committed input is unavailable, report the prerequisite and mark the check unrun. Toolchain updates require a coordinated change to the manifest, development pin, CI, images, and documentation.

Ordinary Rust tests do not need Redis or a running service. Docker differential tests, image tests, and end-to-end benchmarks have additional dependencies. Python tooling needs Python `3.11+`; see [`.github/TOOLING.md`](.github/TOOLING.md) for the full, pinned tool list and commands.

## Implement and test

Keep changes focused. Use rustfmt, address Clippy warnings, and preserve checked arithmetic, bounded resource use, and explicit errors for unsupported options. Do not add Redis behavior by silently accepting an option that does nothing. Update [compatibility](docs/compatibility.md) and [operations](docs/operations.md) in the same PR when the wire or CLI contract changes.

Bug fixes need a regression test that fails before the fix. Major behavior changes must add or update automated tests, including negative cases and error side effects. In particular, test binary/empty inputs, fragmented and pipelined requests, limit boundaries, numeric overflow, TTL boundaries, rejected-write atomicity, and shutdown where relevant. Use deterministic clocks or synchronization rather than guessed sleeps when possible; keep network and concurrency tests bounded.

| Area | Existing test location |
| --- | --- |
| Parser and encoding | Unit/property tests in [`src/protocol.rs`](src/protocol.rs) |
| Storage, accounting, and expiry | Unit/property tests in [`src/db.rs`](src/db.rs) |
| TCP, authentication, limits, and CLI | [`tests/server.rs`](tests/server.rs) |
| Tooling and release policy | [`scripts/test-tooling.py`](scripts/test-tooling.py) and [`scripts/check-release.py`](scripts/check-release.py) |
| Behavioral reference checks | [`scripts/differential.py`](scripts/differential.py), against a disposable Redis 8.10.3 instance |

Run the core checks before requesting review:

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
```

For tooling/image changes, run the corresponding checks in [the tooling runbook](.github/TOOLING.md). [`fuzz/`](fuzz/) is a separate workspace with its own lockfile; follow the runbook's pinned-nightly instructions without changing your global default. Full benchmarks are informational; provide [raw evidence](docs/benchmarks.md) before making performance claims.

## Open a pull request

Use the repository's [PR template](.github/PULL_REQUEST_TEMPLATE.md) to summarize the change, link related issues, describe behavior and risk, and record checks actually run or left unrun. Explain items that do not apply; keep generated benchmark output, binaries, and secrets out of the diff. The template is editable Markdown guidance, not an automated merge gate. Issue form and PR template changes must merge into the default branch before GitHub makes them available.

Use [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/) for commit messages and the final squash title; this is required for Release Please to classify changes. Examples are `fix(protocol): reject oversized bulk headers` and `docs: clarify TTL rounding`. Use `feat:`, `fix:`, `docs:`, `test:`, `ci:`, or another appropriate type; identify breaking changes with `!` or a `BREAKING CHANGE:` footer. This is a contribution requirement, not a claim that CI currently lints titles.

Do not manually bump versions or create release tags in ordinary PRs. [Release Please](docs/releases.md) owns version/changelog updates. The one-time initial `Release-As` footer is a maintainer bootstrap step, not a footer to add to every change.

The aggregate required CI status is **CI required**, defined in [`.github/workflows/ci.yml`](.github/workflows/ci.yml). It checks all Rust matrix, tooling, dependency-policy, and Docker smoke/differential/image-scan jobs; a skipped or canceled dependency does not pass. Repository administrators still need to enable branch protection requiring it. Maintainer review and passing checks are required by project policy; this document does not assert that remote protection settings are already configured.

## Dependencies and attribution

Use the smallest justified dependency set and review maintenance, license, security, MSRV, and transitive impact before adding one. Cargo manifests declare dependencies; the root and fuzz lockfiles record resolutions. CI uses `--locked`. Actions and container images use immutable pins.

[Dependabot](.github/dependabot.yml) checks Cargo (root and fuzz), Actions, Docker, and Compose weekly, grouping patch/minor updates by ecosystem and leaving major updates separate. There is no automerge. The [tooling runbook](.github/TOOLING.md#pins-and-updates) is the source for versions and update verification.

Dependency-policy violations must be resolved or narrowly reviewed before merge/release; do not add blanket advisory ignores. Distributed artifacts need original third-party license/copyright/notice texts generated by [`scripts/notices.py`](scripts/notices.py), with new obligations reviewed by a maintainer. Redis/memtier reference images have their own licenses and are not relicensed as Remik.

## License and authorship

By intentionally submitting a contribution for inclusion, you offer it under [Apache-2.0](LICENSE), the project's outbound license. Submit only work you are authorized to contribute, and disclose the provenance and applicable license of any third-party material. No CLA or copyright assignment is required. Contributors retain their copyright and authorship; keep accurate author and co-author attribution.

Remik is an independent implementation. Do not copy Redis source code into it; use the documented protocol and separately licensed reference server to study observable behavior. The Apache-2.0 grant covers Remik contributions, not Redis or unrelated third-party material.
