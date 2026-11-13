# Implemented compatibility

This is the wire contract for Remik `0.1.0-rc.1`, derived from [`src/protocol.rs`](../src/protocol.rs), [`src/db.rs`](../src/db.rs), and [`src/server.rs`](../src/server.rs). Redis **8.10.3** is the behavioral reference; this document does not claim exhaustive Redis compatibility or compatibility with every Redis client.

## Protocol and client setup

Use TCP, RESP2, database 0, and only the commands and options below. Commands and option tokens are ASCII case-insensitive unless noted. Keys and values are arbitrary bytes, including empty strings, and are case-sensitive.

Requests must be nonempty arrays of non-null bulk strings with CRLF terminators. Inline requests, nested request arrays, null arguments, non-bulk arguments, RESP3 types, and streamed encodings are rejected. Array/bulk lengths are unsigned decimal; leading zeroes are accepted within the 20-digit header cap. Limits are inclusive and apply before execution, including to declared sizes in incomplete frames.

Replies use RESP2 simple strings, errors, signed 64-bit integers, bulk strings, null bulk strings (`$-1\r\n`), and arrays. Pipelining preserves command/reply order within a connection; a pipeline is not a transaction, and other clients can interleave commands. After a protocol failure or `QUIT`, later pipelined commands are not executed.

Minimal encoded request and reply (escape notation, not literal backslashes on the wire):

```text
request: *1\r\n$4\r\nPING\r\n
reply:   +PONG\r\n
```

`HELLO 3` returns `NOPROTO unsupported protocol version`; it never upgrades the connection. Disable client features that require RESP3, transactions, scripts, client tracking, Cluster, Sentinel, or unsupported startup commands. [Resource limits](operations.md#command-line) also constrain client handshakes.

## Command reference

There are **29 top-level commands**. Subcommands do not increase this count. Brackets indicate optional arguments, and `...` indicates repetition. All commands except `AUTH`, `HELLO`, and `QUIT` require an authenticated connection when a password is configured.

| Command syntax | Reply and implemented behavior |
| --- | --- |
| `GET key` | Bulk value, or null if absent/expired |
| `SET key value [NX] [XX] [GET] [EX seconds or PX milliseconds] [KEEPTTL]` | `OK`, null on a skipped write, or the previous bulk/null value with `GET`; restrictions below |
| `MGET key [key ...]` | Array of bulk/null values in request order |
| `MSET key value [key value ...]` | `OK`; atomic admission of the final set of unique keys; last duplicate wins; clears their TTLs |
| `DEL key [key ...]` | Integer count of keys actually removed; repeated keys count only once |
| `EXISTS key [key ...]` | Integer count of live keys in the arguments; repeated live keys each count |
| `TYPE key` | Simple string `string` or `none` |
| `APPEND key value` | Integer resulting byte length; creates a missing key; preserves an existing TTL |
| `STRLEN key` | Integer byte length; 0 for a missing key |
| `INCR key` | Integer result of adding 1 |
| `INCRBY key increment` | Integer result of adding a signed 64-bit increment |
| `DECR key` | Integer result of subtracting 1 |
| `DECRBY key decrement` | Integer result of subtracting a signed 64-bit decrement |
| `EXPIRE key seconds [NX] [XX] [GT] [LT]` | 1 if the expiry changed or the key was deleted; 0 if absent or the condition failed |
| `PEXPIRE key milliseconds [NX] [XX] [GT] [LT]` | As `EXPIRE`, in milliseconds |
| `TTL key` | Integer remaining seconds, rounded to nearest second; -1 without expiry; -2 if absent |
| `PTTL key` | Integer remaining milliseconds, fractional milliseconds truncated; -1 without expiry; -2 if absent |
| `PERSIST key` | 1 if an existing TTL was removed; otherwise 0; does not persist anything to disk |
| `DBSIZE` | Integer count of live keys, excluding logically expired keys |
| `INFO [section ...]` | Bulk report for `server`, `memory`, and/or `keyspace`; details below |
| `FLUSHDB [SYNC]` | `OK`; synchronously destroys the entire dataset and releases table capacity; `ASYNC` is rejected |
| `COMMAND` | Array of legacy six-field command descriptions |
| `PING [message]` | Simple `PONG` without a message; otherwise the exact message as a bulk string |
| `ECHO message` | Exact bulk string |
| `QUIT` | `OK`, then closes the connection; allowed before authentication |
| `AUTH [username] password` | `OK` for the configured password and user `default`; authentication details below |
| `HELLO [2 [AUTH username password] [SETNAME name]]` | Flat RESP2 property array; only protocol 2 and these options are accepted |
| `SELECT index` | `OK` only for integer index 0; no other logical databases exist |
| `CLIENT subcommand ...` | Only `ID`, `GETNAME`, `SETNAME`, and `SETINFO` as documented below |

### String writes and counters

`SET NX` writes only if the key is absent; `XX` writes only if present. Combining them is a syntax error. `GET` returns the previous value even when the condition skips the write. Without `GET`, a skipped write returns null. A successful ordinary `SET` clears the old TTL unless `KEEPTTL` preserves it or `EX`/`PX` supplies a new one.

`EX`/`PX` requires a positive, representable duration. Only one expiry clause is accepted, and it cannot be combined with `KEEPTTL`. Repeated boolean options (`NX`, `XX`, `GET`, `KEEPTTL`) set the same boolean; the incompatible combinations still fail. Absolute-time `EXAT`/`PXAT` and value-comparison options such as `IFEQ`, `IFNE`, `IFDEQ`, and `IFDNE` are unsupported and return a syntax error. Invalid options are checked even if a condition would skip the write.

Counters treat an absent key as zero and preserve an existing TTL. Stored numbers and numeric counter/expiry arguments must be canonical signed decimal in the `i64` range: `0`, positive digits without a leading zero, or `-` followed by a nonzero first digit. `+1`, `01`, `-0`, whitespace, and floats fail. Arithmetic overflow returns an error without changing the value. Like Redis, `DECRBY` rejects the minimum signed 64-bit decrement because its negation cannot be represented, even when the subtraction result would fit. `SELECT` uses separate signed-integer parsing and is not subject to this canonical-number rule.

### Expiration

For `EXPIRE`/`PEXPIRE`, `NX` requires no existing TTL, `XX` requires a TTL, `GT` requires a strictly later deadline, and `LT` requires a strictly earlier deadline. A key without a TTL has an infinite deadline for comparison. `NX` cannot combine with `XX`, `GT`, or `LT`; `GT` and `LT` cannot combine. `XX GT` and `XX LT` are accepted; repeated identical condition tokens are idempotent.

A nonpositive expiry deletes the key only if the conditions permit the change. Malformed or unrepresentable durations and unknown conditions are errors, even for missing keys. Seconds-to-milliseconds overflow is an invalid-expiry error, including for an otherwise nonpositive value. Adding a TTL can return `OOM` because its index record is accounted separately.

Expiration uses monotonic deadlines, passive removal on access, and bounded ordered background cleanup. `PTTL` truncates remaining time to integral milliseconds; `TTL` computes `(remaining_ms + 500) / 1000`. Thus 1500 ms rounds to 2 seconds, and 499 ms to 0; a live key can report `TTL 0`. See [architecture](architecture.md#expiration-and-accounting) for reclamation budgets.

### Authentication and handshake

| Request | Result |
| --- | --- |
| `AUTH password` | Equivalent to username `default` |
| `AUTH default password` | Authenticates this connection; username is case-sensitive |
| `AUTH` with wrong credentials | `WRONGPASS invalid username-password pair`; a valid-arity failed AUTH also clears existing authorization |
| `AUTH` when no password is configured | `ERR AUTH called without any password configured` |
| `HELLO` or `HELLO 2` | Reports RESP2 server properties; requires authorization unless its `AUTH` option succeeds |
| `HELLO 2 AUTH default password SETNAME name` | Validates all options, credentials, and metadata before changing connection state; either option may come first |
| `HELLO 3` or any version token other than exactly `2` | `NOPROTO unsupported protocol version`; connection state stays unchanged |

Each `HELLO` option may appear at most once. Unknown, duplicate, or incomplete options fail. Unlike a failed standalone `AUTH`, failed `HELLO` validation/authentication leaves both authorization and metadata unchanged. Successful `HELLO` returns pairs for `server=remik`, the package `version`, `proto=2`, connection `id`, `mode=standalone`, `role=master`, and an empty `modules` array. The legacy role label does not imply replication support.

### Client metadata

| Request | Reply |
| --- | --- |
| `CLIENT ID` | Positive integer unique to the connection within the process |
| `CLIENT GETNAME` | Bulk name, or null when unset |
| `CLIENT SETNAME name` | `OK`; empty name clears it |
| `CLIENT SETINFO LIB-NAME value` | `OK`; stores connection-local library metadata |
| `CLIENT SETINFO LIB-VER value` | `OK`; stores connection-local library metadata |

Names and metadata are at most 256 bytes, each byte in ASCII `!` through `~`; empty values are allowed, but spaces and control bytes are not. `HELLO SETNAME` uses the same rule. Other subcommands and attributes are explicitly rejected. `CLIENT LIST`, `TRACKING`, `CACHING`, and `NO-EVICT`, among others, are not implemented.

### Introspection

`INFO` without selectors, or with `ALL`/`DEFAULT`, includes all three implemented sections. Explicit selectors are case-insensitive and can be combined. Unknown selectors contribute no section, so `INFO unsupported` returns an empty bulk string; this is section filtering, not evidence that another feature is supported.

`INFO server` identifies `server_name:remik`, `remik_version`, `remik_mode:memory-only`, `protocol:resp2`, `compatibility:string-kv-subset`, and `persistence:none`; it does not invent a `redis_version`. `INFO memory` includes `used_memory`, `maxmemory`, `maxmemory_policy:noeviction`, `memory_accounting:live-entry-estimate-not-rss`, `entry_overhead`, `expiry_overhead`, `expiry_index_entries`, and `oom_expire_budget`. `INFO keyspace` reports live `db0` keys and expiries. These are not the full Redis metrics surface.

`COMMAND COUNT` returns 29. `COMMAND INFO [name ...]` returns all descriptions when no names are supplied, or one description/null per supplied name. Each description is `[name, arity, flags, first_key, last_key, key_step]`, with a single `write` or `readonly` flag. It is legacy metadata, not an ACL model or a complete option grammar. Other `COMMAND` subcommands fail with a syntax error.

## Error semantics

Errors are RESP2 error replies unless the connection must close. Messages are deliberately generic and do not echo request contents. Exact Redis error wording is not a compatibility promise.

| Condition | Remik result |
| --- | --- |
| Unknown top-level command | `ERR unknown command` |
| Wrong database-command arity | `ERR wrong number of arguments for '<name>' command` |
| Wrong connection-command arity | `ERR wrong number of arguments` |
| Invalid/unsupported command option | `ERR syntax error`, or an explicit unsupported-subcommand/attribute error for `CLIENT` |
| Noncanonical/out-of-range number | `ERR value is not an integer or out of range` |
| Counter result overflows | `ERR increment or decrement would overflow` |
| Invalid expiry | `ERR invalid expire time` |
| Dataset admission fails | `OOM command not allowed when used memory > 'maxmemory'` |
| Unauthenticated command | `NOAUTH Authentication required` |
| Invalid password/user | `WRONGPASS invalid username-password pair` |
| Nonzero database | `ERR DB index is out of range` |
| `FLUSHDB ASYNC` or another single non-SYNC argument | `ERR only synchronous FLUSHDB is supported` |
| Malformed frame or decoder limit violation | Attempts `ERR protocol error`, then closes |
| Input buffer fills without a complete frame | Attempts `ERR request frame too large`, then closes |
| Client admission full | Best-effort `ERR max number of clients reached`, then closes; no connection task is admitted |
| Response too large, I/O timeout, or I/O/allocation failure | Closes; no error reply is guaranteed |

Semantic validation and dataset admission happen before writes to live values. A rejected mutation can still reclaim already-expired keys; `MSET` does not partially install live values on admission failure. The OOM text can occur when a proposed write would exceed the budget even though current accounting is below it. Reads and deletions remain usable after an admission OOM.

Execution precedes response-size checking and socket writes. A mutation may succeed but lose its reply through output limits, timeout, or shutdown. There is no rollback or exactly-once retry mechanism. A normal command error leaves the connection available; transport/protocol failures stop processing the rest of that connection's pipeline.

## Explicitly outside this release

There is no RDB/AOF, restart recovery, replication/`WAIT`, Sentinel, Cluster, transactions/`WATCH`, Lua/functions/modules, Pub/Sub, TLS/ACL, runtime `CONFIG`, key scanning, or non-string data types. Extra Redis options are never treated as implemented merely because their parent command exists. The `INFO` selector behavior is described separately above. Consult the [survey](redis-survey.md) for broader Redis families and the [roadmap](roadmap.md) for uncommitted proposals.
