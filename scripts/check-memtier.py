#!/usr/bin/env python3
"""Fail closed on missing/error/empty memtier results, even when it exits zero."""
import argparse
import json
import math
from pathlib import Path


def validate(data, expected_count=None):
    results = data["ALL STATS"]
    totals = results["Totals"]
    for key in ("Errors", "Connection Errors", "Retry Attempts", "Retried Ops"):
        assert totals[key] == 0, f"nonzero {key}: {totals[key]}"
    runtime = results["Runtime"]
    assert runtime["Unanswered requests"] == 0, "unanswered requests"
    assert str(runtime["Interrupted"]).lower() == "false", "interrupted run"
    assert "FATAL_ERROR" not in data, "fatal benchmark error"
    for key in ("Count", "Ops/sec", "Average Latency"):
        assert isinstance(totals[key], (int, float)) and math.isfinite(totals[key])
    assert totals["Count"] > 0 and totals["Ops/sec"] > 0, "empty run"
    assert totals["Average Latency"] >= 0
    assert totals["Misses/sec"] == 0, "preloaded keyspace had misses"
    if expected_count is not None:
        assert totals["Count"] == expected_count, "incomplete preload"
    summary = {key: totals[key] for key in ("Count", "Ops/sec", "Average Latency")}
    summary["Percentile Latencies"] = {key: value for key, value in totals["Percentile Latencies"].items() if key.startswith("p")}
    return summary


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("files", nargs="+", type=Path)
    parser.add_argument("--expected-count", type=int)
    args = parser.parse_args()
    for path in args.files:
        print(json.dumps({"file": str(path), **validate(json.loads(path.read_text()), args.expected_count)}))
