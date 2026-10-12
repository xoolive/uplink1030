# /// script
# requires-python = ">=3.10"
# dependencies = []
# ///
"""Sequential native-binary A/B benchmark; never materializes uncompressed IQ.

Runs identical prefixes in blind and known-address modes and requires byte-exact
JSONL and aggregate-statistics equivalence, including waveform/timing metadata.
"""
import argparse
import json
import re
import subprocess
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--optimized", type=Path, default=Path("target/release/uplink1030"))
    parser.add_argument("--seconds", type=float, default=20)
    parser.add_argument("--output-dir", type=Path, default=Path("performance_native/benchmark"))
    args = parser.parse_args()
    if args.seconds <= 0:
        parser.error("seconds must be positive")
    args.output_dir.mkdir(parents=True, exist_ok=True)
    cloud = Path.home() / "Library/Mobile Documents/com~apple~CloudDocs"
    cases = [
        ("oct5", "gqrx_20261005_112322_1030000000_16000000_fc.raw.zst", Path("opensky_20261005_112322/known_aircraft.csv")),
        ("oct6", "gqrx_20261006_105707_1033326000_16000000_fc.raw.zst", Path("opensky_20261006_105707/known_aircraft.csv")),
    ]
    # Historical binaries filtered by default; newer ones require --known-only.
    known_only_supported = {
        label: "--known-only" in subprocess.run([str(binary.resolve()), "--help"],
            capture_output=True, text=True, check=True).stdout
        for label, binary in [("baseline", args.baseline), ("optimized", args.optimized)]
    }
    summary = []
    for case, filename, known in cases:
        for mode in ["known", "blind"]:
            results = {}
            for label, binary in [("baseline", args.baseline), ("optimized", args.optimized)]:
                prefix = args.output_dir / f"{case}_{mode}_{label}"
                events = prefix.with_suffix(".jsonl")
                stats = prefix.with_suffix(".stats.json")
                log = prefix.with_suffix(".log")
                command = ["/usr/bin/time", "-p", str(binary.resolve()), str(cloud / filename), "--seconds", str(args.seconds), "--stats", "-o", str(events)]
                if mode == "known":
                    command += ["--known-aircraft", str(known)]
                    if known_only_supported[label]:
                        command += ["--known-only"]
                with stats.open("w") as stdout, log.open("w") as stderr:
                    subprocess.run(command, stdout=stdout, stderr=stderr, check=True)
                timings = {key: float(value) for key, value in re.findall(r"^(real|user|sys)\s+(\d+(?:\.\d+)?)$", log.read_text(), re.M)}
                results[label] = {"timing": timings, "stats": json.loads(stats.read_text()), "events": str(events), "events_bytes": events.read_bytes()}
                print(case, mode, label, timings, flush=True)
            assert results["baseline"]["events_bytes"] == results["optimized"]["events_bytes"], f"JSONL mismatch: {case}/{mode}"
            assert results["baseline"]["stats"] == results["optimized"]["stats"], f"stats mismatch: {case}/{mode}"
            for result in results.values():
                del result["events_bytes"]
            row = {"capture": case, "mode": mode, "seconds": args.seconds, "byte_exact_jsonl": True, "results": results,
                   "speedup": results["baseline"]["timing"]["real"] / results["optimized"]["timing"]["real"],
                   "realtime_factor": args.seconds / results["optimized"]["timing"]["real"]}
            summary.append(row)
            print("PASS exact output; speedup", round(row["speedup"], 3), "realtime factor", round(row["realtime_factor"], 3), flush=True)
    (args.output_dir / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")


if __name__ == "__main__":
    main()
