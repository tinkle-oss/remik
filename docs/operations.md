# Configuration and operations

The native `remik` binary starts a TCP server unless given the `healthcheck` subcommand. This reference follows [`src/main.rs`](../src/main.rs) and [`src/config.rs`](../src/config.rs); use `remik --help` for the installed binary's interface.

## Command line

```sh
cargo run --locked -- --bind 127.0.0.1:6379 --max-memory 67108864
cargo run --locked -- healthcheck --bind 127.0.0.1:6379
```

Configuration options are global, so they can appear before or after `healthcheck`. Numeric limits use integer bytes or seconds, not suffixes such as `64mb`. Invalid settings fail startup rather than being clamped or ignored. There is no configuration file, Redis `CONFIG` command, eviction-policy selector, or persistence setting.

| Flag | Default | Accepted value and meaning |
| --- | --- | --- |
| `--bind` | `127.0.0.1:6379` | Numeric IP and port, such as `127.0.0.1:6379` or `[::1]:6379`; not a DNS hostname |
| `--max-memory` | `67108864` | `1..=1099511627776`; accounted dataset budget, not RSS |
| `--max-clients` | `128` | `1..=4096`; concurrent admitted connections, including unauthenticated ones |
| `--max-frame-bytes` | `8388608` | `64..=67108864`; one complete request, including RESP framing |
| `--max-value-bytes` | `1048576` | `1..=16777216`, and at most the frame limit; every bulk argument, including command names, keys, options, and passwords |
| `--max-args` | `1024` | `1..=65536`; includes the command name |
| `--max-output-bytes` | `16777216` | `64..=67108864`; one encoded response, including framing and repeated values |
| `--io-timeout-secs` | `30` | `1..=3600`; idle/partial-request read deadline and whole-response write deadline |
| `--shutdown-timeout-secs` | `5` | `1..=60`; cooperative connection shutdown deadline before aborting tasks |
| `--password-file` | Unset | Read a bounded regular file once at startup; rules below |
| `-h`, `--help` | N/A | Print help; `help` also provides subcommand help |
| `-V`, `--version` | N/A | Print the binary's package version |

Zero is not an unlimited setting. In particular, the CLI rejects `--max-memory 0`.

## Environment variables

| Variable | Meaning |
| --- | --- |
| `REMIK_BIND` | Default for `--bind`; an explicit flag takes precedence |
| `REMIK_PASSWORD_FILE` | Default for `--password-file`; an explicit flag takes precedence |
| `RUST_LOG` | Tracing filter; if absent or invalid, the binary falls back to `remik=info`; logs go to stderr |

These are the runtime environment inputs. There is no `REMIK_PASSWORD`, `REMIK_MAX_MEMORY`, or environment equivalent for the other resource flags. Docker sets `REMIK_BIND=0.0.0.0:6379` inside the container; this differs from the native loopback default. Benchmark-only environment variables are documented in [benchmarks](benchmarks.md#environment-and-results).

## Password files

Have your secret manager create a regular file outside the checkout, readable only by the server's operating-system identity. Pass its path, never its contents:

```sh
cargo run --locked -- --bind 127.0.0.1:6379 \
  --password-file /run/secrets/remik-password
```

The loader accepts at most 4098 file bytes, removes exactly one terminal CRLF or LF, and requires `1..=4096` remaining password bytes. All other bytes, including spaces, NUL, and additional newlines, belong to the password. The configured frame, bulk, and argument limits must accommodate `AUTH` and the password. Loading failures stop startup; there is no insecure fallback.

Only the case-sensitive username `default` is accepted. Authentication state belongs to each connection; see [AUTH and HELLO](compatibility.md#authentication-and-handshake). There is no ACL, per-command authorization, or TLS. All authenticated clients can delete or flush every key. With no password file, connections are immediately authorized for the supported commands.

For Docker, bind-mount the secret file read-only and set `REMIK_PASSWORD_FILE` to its container path. It must be readable by UID `10001`; do not solve that by making it world-readable. The same environment is used by the image healthcheck. Rotating the file requires a server restart, which loses the dataset; there is no hot reload.

## Resource planning

[`src/db.rs`](../src/db.rs) charges each stored entry as follows:

```text
512 + key_length_bytes + value_length_bytes + (1024 if the key has a TTL, else 0)
```

For a 4-byte key and a 128-byte value, the charge is 644 bytes without a TTL or 1668 bytes with a TTL. These are admission charges, not measured allocator usage. `INFO memory` reports this accounting as `used_memory`, including expired entries not yet reclaimed.

Budget separately for retained hash-table capacity, allocator overhead, transient replacements, decoded arguments, response construction, retained read/write buffer capacity, socket buffers, tasks, and the runtime. The default 128-client limit reduces buffer exposure, but it is not proof that every permitted worst-case workload fits a 256 MiB container. Lower client/frame/output limits for hostile or large requests, leave memory headroom, and measure RSS under the intended workload. Container OOM termination still loses all data.

`--max-value-bytes` limits inbound arguments, not accumulated string size. Repeated `APPEND` operations can grow a stored string beyond it, subject to the dataset budget. A later `GET` can then exceed the response limit and close the connection. An `MGET` response counts every encoded occurrence, even when keys repeat.

## Health and shutdown

`remik healthcheck` connects to the configured address, sends `AUTH` if configured, then requires `PONG` from `PING`, all within one I/O timeout. Wildcard listen IPs are translated to the matching IPv4/IPv6 loopback address for the probe. Exit status 0 means the probe passed; failures return a nonzero status. A probe consumes a client slot and checks neither dataset capacity nor durability.

On Linux and macOS, SIGINT or SIGTERM stops acceptance, signals connections to close, and joins their tasks. Tasks that exceed the shutdown deadline are aborted and joined. This is bounded teardown, not a promise to drain pipelines or finish every reply. No data is saved.

## Troubleshooting

| Symptom | Check |
| --- | --- |
| Bind fails | Address syntax, port conflicts, and permission to bind; use loopback unless trusted-network access is intentional |
| `NOAUTH` or `WRONGPASS` | Per-connection authentication, exact password bytes, and username `default` |
| `ERR protocol error`, then disconnect | RESP2 bulk-array encoding and frame/bulk/argument limits; inline requests are invalid |
| Idle or partial request disconnects | The read deadline spans the incomplete request; trickling bytes does not reset it |
| `OOM` | `INFO memory`, live-entry charge, and expiration backlog; no live-key eviction occurs |
| Disconnect without a reply | Response limit, write timeout, shutdown, or allocation/I/O failure; a mutation may already have executed |
| Healthcheck fails in Docker | Client slots, bind/environment consistency, secret-file readability, and server logs |

After an ambiguous disconnect, do not blindly retry non-idempotent writes such as `INCR` or `APPEND`. See [error semantics](compatibility.md#error-semantics). Collect redacted configuration and a minimal reproduction for [GitHub issues](https://github.com/tinkle-oss/remik/issues); send vulnerability details through [SECURITY.md](../SECURITY.md).
