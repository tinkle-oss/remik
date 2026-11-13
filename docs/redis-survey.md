# Redis reference survey

Redis **8.10.3** is Remik's behavioral reference, not its codebase or a compatibility certification. This survey records the wider Redis surface considered when choosing scope. Only the [implemented compatibility contract](compatibility.md), checked against Remik source, describes features available in `0.1.0-rc.1`.

## Reference boundaries

The fixed reference is the [Redis 8.10.3 release](https://github.com/redis/redis/releases/tag/8.10.3) and [tagged source/documentation](https://github.com/redis/redis/tree/8.10.3). The [differential tool](../scripts/differential.py) and [benchmark Compose file](../compose.benchmark.yaml) pin its container digest. Current `latest` Redis documentation is useful for discovery but can describe newer, optional, or beta behavior; check version and build/module availability before adopting a claim.

Redis 8 distributions can include JSON, Search, time-series, and probabilistic modules alongside core commands; a core-only build is not equivalent to every full distribution. The [8.10.3 README](https://github.com/redis/redis/blob/8.10.3/README.md) lists sparse arrays and labels vector sets **beta**. Neither the appearance of a feature in latest documentation nor beta status puts it on Remik's committed roadmap.

Remik is independently implemented under [Apache-2.0](../LICENSE). No Redis code is copied. The reference server, modules, images, and tools retain their own licenses; studying behavior does not relicense their implementation or imply Redis endorsement.

## Data and messaging families

“Studied only” below means absent from Remik and not promised for a release. Representative Redis names identify families, not an exhaustive command inventory.

| Redis family | Capability and questions studied | Remik status |
| --- | --- | --- |
| Strings | Binary values, conditional writes, counters, ranges, and multi-key access | Partial implementation; exact 29-command surface and options in [compatibility](compatibility.md) |
| Hashes | Field/value records, multi-field operations, field TTL, and partial updates | Studied only; no hashes or hash-field expiration |
| Lists | Ordered strings, queues, blocking pops, and trimming | Studied only |
| Sets | Unique members, membership, union, intersection, and difference | Studied only |
| Sorted sets | Scores, rank/range queries, leaderboards, and score updates | Studied only |
| Streams | Append-only records, IDs, consumer groups, pending entries, acknowledgement, and trimming | Studied only; no durable-log claim |
| Pub/Sub | Channel/pattern/sharded subscriptions and transient message delivery | Studied only; not interchangeable with stream replay |
| Geospatial indexes | Coordinate storage, radius/box search, and distance queries | Studied only |
| Bitmaps | Bit access, counting, and bitwise operations over strings | Studied only; storing a binary string does not implement bit commands |
| Bitfields | Packed signed/unsigned integer fields and overflow policies | Studied only |
| HyperLogLog | Approximate distinct cardinality (`PF*`) | Studied only |
| Bloom and Cuckoo filters | Approximate membership; different deletion/counting tradeoffs (`BF.*`, `CF.*`) | Studied only |
| Count-min sketch | Approximate frequency counts (`CMS.*`) | Studied only |
| Top-K | Approximate frequent-item tracking (`TOPK.*`) | Studied only |
| t-digest | Approximate quantiles (`TDIGEST.*`) | Studied only |
| JSON | Structured document/path operations (`JSON.*`) | Studied only; opaque JSON bytes can be stored as strings |
| Search and vector query engine | Indexing hash/JSON documents, full-text/numeric/geo/vector queries, and aggregation (`FT.*`) | Studied only; no secondary indexes |
| Vector sets | Similarity search over embeddings (`V*`); beta in the tagged reference README | Studied only; distinct from Search vector indexes |
| Time series | Timestamped samples, retention, compaction, and aggregation (`TS.*`) | Studied only |
| Arrays | Sparse index-addressable string sequences (`AR*`), distinct from lists and RESP reply arrays | Studied only |

## Cache policy and expiration

The [current Redis eviction reference](https://redis.io/docs/latest/develop/reference/eviction/) lists **10 policies**:

| Policy group | Names | Selection rule |
| --- | --- | --- |
| No eviction | `noeviction` | Reject memory-growing commands when memory cannot be admitted |
| All keys | `allkeys-lru`, `allkeys-lfu`, `allkeys-lrm`, `allkeys-random` | Choose among all keys by approximate recency of use, frequency, recency of modification, or random selection |
| Expiring keys only | `volatile-lru`, `volatile-lfu`, `volatile-lrm`, `volatile-random` | Use those selection rules among TTL-bearing keys |
| Shortest remaining TTL | `volatile-ttl` | Choose expiring keys with the shortest remaining lifetime |

Redis LRU/LFU/LRM are approximations with sampling/tuning tradeoffs; LRM tracks writes rather than reads. Volatile policies cannot reclaim persistent keys and behave like noeviction if there are no eligible keys. Eviction under memory pressure is different from expiration when a deadline passes.

Redis uses passive expiration on access and active background expiry work, and supports key TTL and hash-field TTL. Wall-clock/absolute-expiry, persistence, replication, and query-index visibility introduce additional semantics. Remik implements only the documented relative key TTL subset, with monotonic time and an ordered bounded cleanup index. It supports only fixed **noeviction** admission, with its own [accounting formula](architecture.md#expiration-and-accounting); Redis's allocator-based memory figures are not equivalent.

## Durability and distributed operation

| Redis area | Semantics and operational costs studied | Remik status |
| --- | --- | --- |
| RDB snapshots | Point-in-time recovery, snapshot interval loss window, fork/copy-on-write cost, backups, and restore verification | Absent |
| AOF | Write log/replay and rewrite; `appendfsync always`, `everysec`, or `no` trade throughput for durability and loss windows | Absent |
| Combined persistence and disaster recovery | Recovery source selection, corruption/truncation handling, off-host backups, and tested restoration; replication alone is not a backup | Absent; every restart loses all keys |
| Replication | Asynchronous primary/replica propagation, full/partial synchronization, lag, and possible acknowledged-write loss at failover | Absent |
| `WAIT` | Waits for replica acknowledgements; does not turn Redis into a CP/strongly consistent system or guarantee loss-free failover | Absent |
| Sentinel | Failure detection, promotion, service discovery, and operational quorum assumptions | Absent |
| Cluster | Hash slots, routing/redirections, multi-key restrictions, resharding/migration, and failover | Absent; one database on one process |

Neither atomic command execution nor a successful socket reply establishes durability. Future Remik persistence or replication would require a separate failure model, recovery format, compatibility policy, and crash/failover tests before any such claim.

## Execution, security, and administration

| Redis area | Scope studied | Remik status |
| --- | --- | --- |
| Transactions | `MULTI`/`EXEC`/`DISCARD` serialize a queued group; runtime errors do not provide SQL-style rollback | Absent; pipelines are not transactions |
| Optimistic concurrency | `WATCH` detects changes before conditional `EXEC`; newer string comparison options offer narrower operations | Absent beyond documented `SET NX`/`XX` |
| Scripts and functions | Server-side Lua, atomic execution, script/function lifecycle, time/resource limits, and replication of effects | Absent |
| Modules | Extension APIs, new types/commands, ABI and licensing obligations | Absent |
| RESP and clients | RESP2/RESP3 types, handshakes, pipelining, tracking, and push messages | Only RESP2 bulk-array requests and documented handshake/metadata subset |
| Authentication and authorization | ACL users/categories/key/channel rules, password rotation, and TLS transport | Default-user password-file AUTH only; no ACL or TLS |
| Limits and metrics | Client/query/output limits; `INFO`, latency, memory, slow logs, and command statistics | Fixed startup limits, limited `INFO`, and stderr tracing; no full Redis observability surface |
| Configuration and administration | `CONFIG`, database/keyspace administration, scanning, diagnostics, shutdown, and operational access control | Limited `SELECT 0`, `DBSIZE`, synchronous `FLUSHDB`, and legacy `COMMAND`; no runtime `CONFIG` |

## Sources

Use these upstream sources for Redis concepts; use Remik's [source-linked contract](compatibility.md) for actual support:

- [Redis 8.10.3 release](https://github.com/redis/redis/releases/tag/8.10.3) and [tagged README](https://github.com/redis/redis/blob/8.10.3/README.md).
- [RESP specification](https://redis.io/docs/latest/develop/reference/protocol-spec/) and [command reference](https://redis.io/docs/latest/commands/).
- [Data types](https://redis.io/docs/latest/develop/data-types/), [arrays](https://redis.io/docs/latest/develop/data-types/arrays/), [vector sets](https://redis.io/docs/latest/develop/data-types/vector-sets/), and [Search/query](https://redis.io/docs/latest/develop/ai/search-and-query/).
- [Eviction policies](https://redis.io/docs/latest/develop/reference/eviction/) and [expiration behavior](https://redis.io/docs/latest/commands/expire/).
- [Persistence and recovery](https://redis.io/docs/latest/operate/oss_and_stack/management/persistence/) and [replication/WAIT limitations](https://redis.io/docs/latest/operate/oss_and_stack/management/replication/).
- [Transactions and WATCH](https://redis.io/docs/latest/develop/using-commands/transactions/) and [programmability](https://redis.io/docs/latest/develop/programmability/).

This survey is a category-level scope review, not a claim that every command, client, module, or failure mode has been tested. Future work needs an explicit proposal and evidence under [CONTRIBUTING.md](../CONTRIBUTING.md).
