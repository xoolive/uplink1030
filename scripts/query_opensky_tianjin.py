# /// script
# requires-python = ">=3.11"
# dependencies = ["pyopensky>=2.16", "pandas"]
# ///
"""Query OpenSky aircraft around the Tianjin 1030 MHz capture."""
from datetime import datetime, timedelta, timezone
from pathlib import Path
from pyopensky.trino import Trino

START = datetime(2026, 7, 30, 5, 45, 41, tzinfo=timezone.utc)
DURATION = 31_887_796_888 / (20_000_000 * 8)
STOP = START + timedelta(seconds=DURATION)
BOUNDS = (116.0, 38.3, 119.0, 40.5)  # west, south, east, north
OUT = Path("opensky_20260730_054541")
OUT.mkdir(exist_ok=True)
start, stop = START.timestamp(), STOP.timestamp()
hour = int(start // 3600 * 3600)

sql = f"""
SELECT time, icao24, trim(callsign) AS callsign, lat, lon,
       baroaltitude, geoaltitude, velocity, heading, vertrate,
       onground, squawk, lastcontact
FROM state_vectors_data4
WHERE hour = {hour}
  AND time BETWEEN {int(start)} AND {int(stop) + 1}
  AND lon BETWEEN {BOUNDS[0]} AND {BOUNDS[2]}
  AND lat BETWEEN {BOUNDS[1]} AND {BOUNDS[3]}
ORDER BY time, icao24
"""
print(f"UTC interval: {START.isoformat()} – {STOP.isoformat()} ({DURATION:.3f} s)")
states = Trino().query(sql, cached=True)
states.to_csv(OUT / "state_vectors.csv", index=False)
if not states.empty:
    aircraft = (states.sort_values("time").groupby("icao24", as_index=False).agg(
        callsign=("callsign", "last"), first_time=("time", "min"),
        last_time=("time", "max"), lat=("lat", "last"), lon=("lon", "last"),
        baroaltitude=("baroaltitude", "last"), onground=("onground", "last"),
        squawk=("squawk", "last")))
else:
    aircraft = states
aircraft.to_csv(OUT / "aircraft_opensky.csv", index=False)
print(f"OpenSky: {len(states)} state vectors, {len(aircraft)} aircraft")
