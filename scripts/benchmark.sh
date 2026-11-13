#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
timeout_cmd=$(command -v timeout || command -v gtimeout) || { printf '%s\n' 'Install coreutils (timeout/gtimeout required).' >&2; exit 1; }
BENCH_UID=$(id -u)
BENCH_GID=$(id -g)
export BENCH_UID BENCH_GID
mkdir -p benchmark-results
results=$(mktemp -d benchmark-results/run.XXXXXX)
project="remik-bench-$(basename "$results" | tr '[:upper:].' '[:lower:]-')"
compose=(docker compose -p "$project" -f compose.benchmark.yaml)
cleanup() {
  "${compose[@]}" logs --no-color > "$results/servers.log" 2>&1 || true
  "${compose[@]}" down --volumes --remove-orphans
}
trap cleanup EXIT
python3 scripts/notices.py --target x86_64-unknown-linux-gnu
python3 scripts/benchmark-inputs.py capture "$results"
"${compose[@]}" config > "$results/compose.yaml"
{ uname -a; docker version; docker info; cargo --version; git rev-parse --verify HEAD || printf 'uncommitted (no HEAD)\n'; } > "$results/environment.txt"
"${compose[@]}" build remik
python3 scripts/benchmark-inputs.py verify "$results"
common=(--protocol=redis --threads=2 --clients=8 --pipeline=8 --data-size=128
  --key-prefix=bench: --key-minimum=1 --key-maximum=10000 --key-pattern=R:R
  --ratio=1:10 --distinct-client-seed --hide-histogram '--print-percentiles=50,95,99,99.9' --retry-on-error --max-retries=0
  --connection-timeout=5 --request-timeout=10 --connection-stage-timeout=15)
run_memtier() {
  local service=$1 phase=$2
  shift 2
  "$timeout_cmd" 120 "${compose[@]}" run --rm --no-deps -T memtier \
    "${common[@]}" --server="$service" "$@" \
    --json-out-file="/results/$(basename "$results")/$service-$iteration-$phase.json" \
    > "$results/$service-$iteration-$phase.log" 2>&1
  if [[ $phase == preload ]]; then
    python3 scripts/check-memtier.py "$results/$service-$iteration-$phase.json" --expected-count 10000
  else
    python3 scripts/check-memtier.py "$results/$service-$iteration-$phase.json"
  fi
}
for iteration in 1 2 3; do
  services=(remik redis)
  if (( iteration % 2 == 0 )); then services=(redis remik); fi
  for service in "${services[@]}"; do
    "${compose[@]}" up -d --wait --wait-timeout 45 "$service"
    # memtier's allkeys uses max-min, not the inclusive keyspace size.
    run_memtier "$service" preload --threads=1 --clients=1 --pipeline=1 --ratio=1:0 --key-pattern=S:S --requests=10000
    run_memtier "$service" warmup --test-time=3
    run_memtier "$service" measured --test-time=10
    "${compose[@]}" logs --no-color "$service" >> "$results/$service.log"
    "${compose[@]}" rm --stop --force --volumes "$service"
  done
done
python3 scripts/benchmark-inputs.py verify "$results"
python3 scripts/check-memtier.py "$results/"*-measured.json > "$results/summary.jsonl"
printf 'Results: %s (informational; no hosted-runner performance gate)\n' "$results"
