# /// script
# requires-python = ">=3.11"
# dependencies = ["pandas"]
# ///
"""Summarize locally received Tianjin Mode S downlinks."""
import json
from pathlib import Path
import pandas as pd

SOURCE = Path.home() / "output_tianjin.jsonl"
OUT = Path("opensky_20260730_054541")
OUT.mkdir(exist_ok=True)
rows = [json.loads(line) for line in SOURCE.open() if line.strip()]
df = pd.DataFrame(rows)
df.to_csv(OUT / "local_downlinks.csv", index=False)
summary_rows = []
for icao24, group in df.dropna(subset=["icao24"]).groupby("icao24"):
    counts = group["df"].astype(str).value_counts()
    last = group.sort_values("timestamp").iloc[-1]
    callsigns = group.get("callsign", pd.Series(dtype=object)).dropna()
    summary_rows.append({
        "icao24": icao24.lower(),
        "callsign": callsigns.iloc[-1].strip() if len(callsigns) else "",
        "first_time": group.timestamp.min(), "last_time": group.timestamp.max(),
        "frames": len(group),
        **{f"df{k}": int(v) for k, v in counts.items()},
        "last_lat": last.get("latitude"), "last_lon": last.get("longitude"),
        "last_altitude": last.get("altitude"),
    })
summary = pd.DataFrame(summary_rows).fillna("").sort_values("icao24")
summary.to_csv(OUT / "aircraft_local.csv", index=False)
print(summary.to_string(index=False))
print(f"\n{len(df)} frames, {len(summary)} addressed aircraft")
