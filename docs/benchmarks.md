# Benchmarks

Remik has a Criterion target for in-process measurements and a Docker/memtier procedure for end-to-end comparison with Redis **8.10.3**. No validated performance result is claimed here: benchmark configuration, compilation, or a smoke test alone does not establish throughput, latency, or superiority over Redis.

## Run the configured benchmarks

Use a complete checkout with the root/fuzz lockfiles, `LICENSE`, source, and the `core` benchmark source in place. [`Cargo.toml`](../Cargo.toml) declares `[[bench]] name = "core"` with `harness = false` and Criterion `0.8`. The benchmark source and selected workload must be reviewed before interpreting results.

```sh
# Compile benchmarks; this does not measure performance.
cargo bench --locked --no-run
# Execute Criterion's short test mode; also not a performance measurement.
cargo bench --locked --bench core -- --test
# Run Criterion measurements.
cargo bench --locked --bench core
# Run the separate memtier comparison.
scripts/benchmark.sh
```

The end-to-end script needs Python `3.11+`, Docker Engine/Desktop with Compose v2, the project Rust toolchain, and coreutils `timeout` (`gtimeout` is accepted on macOS). Ports `16379` and `16380` must be free. Do not run two copies concurrently. It builds the local Remik image after notice generation, uses a unique Compose project and output directory, and removes its servers and disposable reference volumes on exit.

## End-to-end workload

The executable procedure is [`scripts/benchmark.sh`](../scripts/benchmark.sh); resource definitions and immutable image pins live in [`compose.benchmark.yaml`](../compose.benchmark.yaml). Do not substitute an unpinned Redis or memtier image when presenting a comparison as this procedure.

| Setting | Procedure |
| --- | --- |
| Servers | Remik and digest-pinned Redis 8.10.3, only one running at a time |
| Server quotas | Each gets 1 CPU, 256 MiB container RAM, 64 MiB dataset budget, and 128 maximum clients |
| Redis durability/I/O | RDB saving and AOF disabled; one I/O thread; noeviction |
| Generator | Digest-pinned memtier 2.6.1; 2 CPUs and 256 MiB RAM |
| Dataset | 10,000 keys, `bench:` prefix, integer range 1 through 10,000, 128-byte values |
| Preload | Fresh server for every trial; sequential complete keyspace, 1 thread/client, pipeline 1, SET-only, exactly 10,000 requests |
| Warmup/measurement | 3 seconds warmup, then 10 seconds measured, for each server in each of 3 trials |
| Measured clients | 2 threads, 8 clients per thread, pipeline 8; SET:GET ratio 1:10 |
| Key selection | Random key pattern, deterministic distinct client seeds; no `--randomize` |
| Order | Remik then Redis in trials 1 and 3; Redis then Remik in trial 2 |
| Network | Both accessed by memtier over the same Docker bridge path |

The script fixes `--protocol=redis`. Keep that mode when reproducing this workload because memtier's other protocol modes can add negotiation commands. Remik supports only the restricted [RESP2 handshake](compatibility.md#authentication-and-handshake), not RESP3.

Retries are exposed with `--retry-on-error --max-retries=0`. Timeouts bound connection/request stages and each memtier process. Equal CPU quotas do not isolate physical cores; the host scheduler, Docker VM on macOS, generator saturation, thermal behavior, and shared runners can dominate a result. The 10,000-key dataset is intentionally below both budgets; this is not an eviction-pressure benchmark, and equal dataset budgets do not imply equivalent memory accounting or RSS.

## Environment and results

`scripts/benchmark.sh` sets and exports `BENCH_UID` and `BENCH_GID` from the invoking host user so memtier can write results into the bind mount. [`compose.benchmark.yaml`](../compose.benchmark.yaml) defaults each to `1000` when invoked directly. They are benchmark file-ownership inputs, not Remik runtime settings. There are no implemented environment knobs for trial count, duration, key count, or quotas; change the source in a reviewed comparison rather than inventing `BENCH_*` overrides.

Each run writes a fresh `benchmark-results/run.*` directory containing raw per-server/trial/phase JSON and logs, server logs, rendered Compose settings, `environment.txt`, and `summary.jsonl`. Source provenance includes Git HEAD when available, tracked/untracked status, local input patches, and per-file input hashes. The script checks those hashes after building and after measurements and rejects changed inputs. The environment capture includes OS, Docker version/info, and Cargo version; it is not a complete machine inventory. Record CPU model, available RAM, power settings, and relevant background load alongside a report. Review files for host-identifying information before sharing.

[`scripts/check-memtier.py`](../scripts/check-memtier.py) requires parseable JSON, finite positive request counts and throughput, nonnegative average latency, zero errors/connection errors/retries/retried operations, zero unanswered requests, no interruption/fatal result, and zero GET misses. Preload also requires exactly 10,000 requests. A zero process exit status is not sufficient; memtier's `allkeys` count excludes one inclusive endpoint, so the script uses an explicit preload request count.

## Interpret and publish evidence

Report all trials, including latency percentiles and unsuccessful runs, with raw validated output, exact source revision/local changes, tool/image versions, resource settings, and environment metadata. Separate Criterion's in-process costs from memtier's TCP, encoding, client, and container costs. Do not turn one workload into a general performance or production-readiness claim.

[`.github/workflows/benchmark.yml`](../.github/workflows/benchmark.yml) runs the full procedure weekly and manually on trusted main, retaining Criterion and memtier artifacts for 30 days. PR CI compiles benchmarks and runs Criterion test mode only. There is no PR throughput/latency threshold and no validated baseline published by this document. See [the tooling runbook](../.github/TOOLING.md#benchmarks) for operational prerequisites and pins.
