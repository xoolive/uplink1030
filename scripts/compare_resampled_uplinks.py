# /// script
# requires-python = ">=3.10"
# dependencies = []
# ///
"""Compare native event streams by exact raw/target and one-to-one sample time."""
import argparse
import collections
import json
import statistics
from pathlib import Path


def load(path):
    rows = [json.loads(line) for line in path.open()]
    for row in rows:
        rate = row.get("decode_sample_rate_hz", row["sample_rate_hz"])
        assert abs(row["seconds"] - row["detection"]["p6_sample"] / rate) < 1e-12
        if "input_p6_sample" in row:
            assert abs(row["input_p6_sample"] - row["seconds"] * row["sample_rate_hz"]) < 1e-8
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--fixture", type=Path, action="append", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--tolerance-us", type=float, default=3)
    args = parser.parse_args()
    if args.tolerance_us <= 0:
        parser.error("tolerance must be positive")
    baseline = load(args.baseline)
    index = collections.defaultdict(list)
    for n, row in enumerate(baseline):
        index[(row["message"]["icao24"], row["message"]["raw"])].append((row["seconds"], n))
    summary = []
    for path in args.fixture:
        rows = load(path)
        used, pairs, extras = set(), [], []
        for row in rows:
            key = (row["message"]["icao24"], row["message"]["raw"])
            candidates = [(abs(t - row["seconds"]), t, n) for t, n in index.get(key, [])
                          if n not in used and abs(t - row["seconds"]) <= args.tolerance_us * 1e-6]
            if candidates:
                _, t, n = min(candidates)
                used.add(n)
                pairs.append((row, (row["seconds"] - t) * 1e6))
            else:
                extras.append(row)
        missing = [row for n, row in enumerate(baseline) if n not in used]
        deltas = [delta for row, delta in pairs]
        result = {"fixture": str(path), "events": len(rows), "baseline_events": len(baseline),
                  "matched_baseline": len(pairs), "baseline_recovery_fraction": len(pairs) / len(baseline) if baseline else None,
                  "new_relative_to_baseline": len(extras), "missed_baseline": len(missing),
                  "matched_uf_counts": dict(collections.Counter(row["message"]["type"] for row, delta in pairs)),
                  "missed_uf_counts": dict(collections.Counter(row["message"]["type"] for row in missing)),
                  "new_uf_counts": dict(collections.Counter(row["message"]["type"] for row in extras)),
                  "timing_delta_us_median": statistics.median(deltas) if deltas else None,
                  "timing_delta_us_min": min(deltas) if deltas else None,
                  "timing_delta_us_max": max(deltas) if deltas else None,
                  "decision_confidence_median": statistics.median(row["decision_confidence"] for row in rows) if rows else None,
                  "one_to_one": True, "exact_target_and_raw": True, "tolerance_us": args.tolerance_us}
        summary.append(result)
        print(json.dumps(result, indent=2))
    args.output.write_text(json.dumps(summary, indent=2) + "\n")


if __name__ == "__main__":
    main()
