# Roadmap

Remik's current scope is a single-process, memory-only cache with a small RESP2 string/key-value subset. `0.1.0-rc.1` is experimental. This roadmap separates implemented behavior from work that needs validation or a proposal; it commits to no delivery dates.

## Current implementation

The [compatibility contract](compatibility.md) records the 29 top-level commands, their restricted options, default-user AUTH, and client metadata. [Operations](operations.md) documents finite limits and dataset admission. [Architecture](architecture.md) describes ordered expiration and bounded cleanup. None of these provides persistence or distributed availability.

## Candidate validation priorities

Before making broader stability or performance claims, collect evidence for:

- Boundary and regression coverage for protocol limits, write-admission atomicity, TTL transitions, authentication state, and shutdown/backpressure.
- Repeatable [benchmark results](benchmarks.md) with raw outputs and host metadata, plus RSS and transient-memory measurements under client/output pressure.
- Client startup compatibility using the supported RESP2 contract, without silently accepting unsupported commands/options.
- A completed [release bootstrap](releases.md), native and multiarch artifact verification, and reviewed dependency/license notices.
- Administrator-enabled private reporting, branch protection, scoped release credentials, and an explicit review of the [OSPS Baseline v2026.08.28](https://baseline.openssf.org/versions/2026-08-28.html) gaps. No compliance claim is made.

These priorities are acceptance topics, not evidence that the checks have already passed or settings have been enabled.

## Proposals, not implemented features

| Topic | Evidence a proposal must address |
| --- | --- |
| Additional string commands or data types | Use case, precise supported options/errors, memory model, and reference/regression tests |
| Eviction policies | Selection semantics, expiry interaction, workload fit, accounting, latency, and benchmark evidence |
| Persistence/recovery | Data-loss contract, on-disk format/versioning, crash consistency, restore procedure, and failure tests |
| Replication or clustering | Consistency/failover guarantees, routing, resynchronization, lost-write scenarios, and operational ownership |
| TLS or richer authorization | Threat model, identity/secret lifecycle, access enforcement, dependencies, and interoperability |
| Parallel database execution | Ordering guarantees, multi-key atomicity, resource bounds, complexity, and measured benefit |

The [Redis survey](redis-survey.md) is a research inventory, not a commitment to recreate all Redis families, modules, or beta features. Bring major changes through the [contribution proposal process](../CONTRIBUTING.md#discuss-the-change) before implementing them.
