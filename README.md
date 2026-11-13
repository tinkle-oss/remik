# Remik

Remik is an independently written Rust server for a small Redis-compatible string/key-value subset. Version `0.1.0-rc.1` is an experimental prerelease for cache-first workloads on trusted networks, not a stable or production-readiness claim.

All data lives in memory. Every stop, crash, or restart loses the entire dataset. There is no persistence, replication, TLS, or ACL system. Keep the authoritative copy of your data elsewhere.

## Quick start

From a complete checkout, use the Rust `1.99.1` toolchain pinned in [`rust-toolchain.toml`](rust-toolchain.toml), with a native linker. Linux and macOS are the native targets; macOS needs Xcode Command Line Tools. The declared MSRV is Rust `1.99` (`1.99.0` in CI).

```sh
cargo run --locked -- --bind 127.0.0.1:6379
```

In another terminal, use an installed `redis-cli` in RESP2 mode:

```sh
redis-cli -2 -h 127.0.0.1 -p 6379 PING
# PONG
redis-cli -2 -h 127.0.0.1 -p 6379 SET greeting hello EX 60
# OK
redis-cli -2 -h 127.0.0.1 -p 6379 GET greeting
# hello
```

Remik accepts RESP2 arrays of bulk strings, including binary keys and values, and supports pipelining. It does not accept inline commands such as raw `PING\r\n` over a socket, or RESP3. Configure clients for database 0 and the [implemented command subset](docs/compatibility.md); a Redis client library is not automatically compatible with every server feature.

## Run with Docker

Build locally first. Generate dependency notices before the image build; select the notice target for the image platform (`x86_64-unknown-linux-gnu` for amd64, `aarch64-unknown-linux-gnu` for arm64).

```sh
python3 scripts/notices.py --target x86_64-unknown-linux-gnu
docker build -t remik:local .
docker run --rm --name remik --read-only --cap-drop=ALL \
  --security-opt=no-new-privileges:true --cpus=1 --memory=256m \
  --pids-limit=128 -p 127.0.0.1:6379:6379 \
  remik:local --max-memory 67108864 --max-clients 128
```

Alternatively, after generating notices, run `docker compose up --build -d --wait` with [`compose.yaml`](compose.yaml), and stop it with `docker compose down`. Both examples publish only to host loopback. The image runs as UID/GID `10001:10001`; Compose uses a read-only root filesystem and no data volume. Container peers can still reach the server, so use a trusted Docker network.

The release workflow targets Linux `amd64` and `arm64` images. **Only after publication**, the candidate image name will be `ghcr.io/tinkle-oss/remik:0.1.0-rc.1`, with a moving `:rc` alias and no `:latest` alias. This README does not assert that either image or a downloadable release already exists. See [release operations](docs/releases.md) for setup and verification.

## Configure and operate

```sh
cargo run --locked -- --help
cargo run --locked -- healthcheck --bind 127.0.0.1:6379
```

Defaults include a 64 MiB accounted dataset budget, 128 clients, an 8 MiB request frame, a 1 MiB limit on **every bulk argument**, and a 16 MiB encoded response limit. `--max-memory` is not a process/RSS cap. Remik rejects writes that do not fit; it never evicts live keys. See [configuration and operations](docs/operations.md) for every flag, environment variable, password-file rules, and resource caveats.

Stop a native server with Ctrl-C or SIGTERM. Shutdown closes client connections without saving data. Password authentication is optional and unencrypted; it does not make an untrusted network safe. Read the [security policy](SECURITY.md) before changing the bind address.

## Documentation

| Document | Purpose |
| --- | --- |
| [Compatibility](docs/compatibility.md) | All 29 commands, supported options, responses, and errors |
| [Operations](docs/operations.md) | Configuration, authentication, healthchecks, and troubleshooting |
| [Architecture](docs/architecture.md) | Request flow, memory accounting, and expiration |
| [Redis survey](docs/redis-survey.md) | Redis 8.10.3 reference and broader features studied, not implemented |
| [Benchmarks](docs/benchmarks.md) | Criterion and memtier procedures; no validated performance claim yet |
| [Releases](docs/releases.md) | Candidate versions, artifacts, verification, and maintainer setup |
| [Roadmap](docs/roadmap.md) | Current scope and proposals requiring evidence |
| [Contributing](CONTRIBUTING.md) | Development checks, regression tests, and review process |
| [Tooling contract](.github/TOOLING.md) | Canonical CI, dependency pins, fuzzing, and release runbook |

For bugs and usage questions, use [GitHub issues](https://github.com/tinkle-oss/remik/issues). For vulnerabilities, follow [private reporting instructions](SECURITY.md#report-a-vulnerability), not public issues. [Maintainers](MAINTAINERS.md) provide best-effort support, without an on-call service or support SLA.

## License and provenance

Copyright 2026 Remik contributors. Remik's independent code is licensed under [Apache-2.0](LICENSE). Redis **8.10.3** is a behavioral reference and a separately licensed test/benchmark dependency; no Redis source code is copied into Remik. Dependency license and notice obligations remain separate; distributed binaries and images include generated third-party notices. See the [survey's sources](docs/redis-survey.md#sources) and [contribution rules](CONTRIBUTING.md#license-and-authorship).
