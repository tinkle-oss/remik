# Architecture

Remik has one memory-only string database and a bounded TCP/RESP2 interface. This overview describes the current implementation; it is not a design for persistence or a distributed database.

## Request flow

```text
operator -> CLI/config validation -> bound TCP listener
                                      |
client -> admission -> bounded decode -> connection dispatch
                                      |          |
                                      |          +-> AUTH/HELLO/CLIENT state
                                      +-> shared database -> bounded reply -> client
                                                  ^
                                      ordered expiry cleanup
```

| Module | Responsibility |
| --- | --- |
| [`src/main.rs`](../src/main.rs) | CLI/environment, tracing, password-file loading, healthcheck dispatch, and Unix shutdown signals |
| [`src/config.rs`](../src/config.rs) | Validated limits and bounded secret-file loading |
| [`src/protocol.rs`](../src/protocol.rs) | Bounded RESP2 bulk-array decoding and response encoding |
| [`src/server.rs`](../src/server.rs) | Admission, per-connection state, I/O deadlines, healthchecks, and task shutdown |
| [`src/db.rs`](../src/db.rs) | String operations, write admission, ordered expiration, and command metadata |
| [`tests/server.rs`](../tests/server.rs) | TCP, concurrency, limits, authentication, and CLI regression coverage |

The server owns the database and connection tasks. Clients have no filesystem access through the command interface. The operator supplies settings and an optional password file; the process reads that file at startup. The runtime interfaces are TCP, CLI/environment, stderr logs, and operating-system shutdown signals. Build/release tools separately contact package registries and GitHub; they are not server data paths.

## Concurrency and ordering

[`src/server.rs`](../src/server.rs) uses Tokio tasks for connections and serializes database commands with a shared mutex. No database lock is held across network I/O. Individual commands, including multi-key `MSET`, execute without another database command interleaving. Pipelines do not reserve the lock across commands and are not transactions.

A semaphore limits admitted clients, including idle and unauthenticated connections. Excess connections receive a short, best-effort rejection without spawning another client task. A connection executes and writes one reply at a time and yields after 32 pipelined commands. Large command work can still delay others; the implementation has no sharding or parallel database execution.

The parser validates a whole command before allocating owned arguments. Stored key/value bytes are copied into database-owned storage, so a small key does not pin an oversized request allocation. Encoded response size is checked before growing the output buffer, but after execution; a disconnect does not prove that a write failed.

## Expiration and accounting

[`src/db.rs`](../src/db.rs) stores values in a hash map and deadlines in an ordered index. Each expiring key has exactly one index record; replacement, deletion, and `PERSIST` remove its old record immediately. Cleanup visits due deadlines in order instead of scanning the full keyspace or retaining stale timer records.

Expiration has three paths:

- Access to a target key removes it if its monotonic deadline has passed.
- The server schedules cleanup every 100 ms, removing at most 256 due keys per tick; missed ticks are skipped. This is a work bound, not a hard expiration-latency guarantee.
- A write that does not initially fit reclaims at most 64 unrelated due keys before checking admission again. It can still fail while a larger expiration backlog remains; a later retry after cleanup can fit.

`DBSIZE` and keyspace reports count live entries without an unbounded removal sweep. Counting still scans entries/index records under the database lock. An expired, unaccessed entry can remain charged until cleanup, so `used_memory` and live-key counts need not drop together. TTL response rounding is specified in [compatibility](compatibility.md#expiration).

The constants `ENTRY_OVERHEAD=512` and `EXPIRY_OVERHEAD=1024` in `src/db.rs` define the admission formula:

```text
entry charge = 512 + key bytes + value bytes + (1024 when expiring)
```

Writes are checked against the final accounted total, with replaced entries subtracted. `MSET` deduplicates keys before charging; the last value wins. Adding a TTL consumes its fixed index charge; removing it releases that charge. There is no live-key eviction.

Accounting estimates entry/index allocations and spare buckets conservatively; it does not measure total process memory. Retained table capacity after deletion, transient copies during replacement, decoded arguments, response objects, retained connection buffers, allocator behavior, and runtime/socket memory need separate headroom. Explicit `FLUSHDB` drops the tables as well as the entries. See [resource planning](operations.md#resource-planning) before assigning container limits.

## Failure and trust boundaries

Untrusted TCP bytes pass through finite frame, argument, output, client, and I/O limits. Password comparisons use a fixed public padding bound; protocol diagnostics avoid reflecting input. These measures do not provide TLS, ACLs, rate limiting, tenant isolation, or a denial-of-service guarantee. Every authorized connection can access and destroy the entire dataset.

Shutdown stops acceptance, cancels connection work, and joins tasks, aborting overdue tasks after the configured deadline. Neither successful replies nor clean shutdown provide durability: there is no journal, snapshot, replica, or recovery path. A new process starts empty. The experimental Rust API exported by [`src/lib.rs`](../src/lib.rs) is also pre-1.0 and has no stability guarantee.

## Further reading

Read [operations](operations.md) for limits and deployment, [compatibility](compatibility.md) for exact protocol behavior, and [SECURITY.md](../SECURITY.md) for threat assumptions and reporting. [Benchmarks](benchmarks.md) separates in-process measurements from network/container measurements.
