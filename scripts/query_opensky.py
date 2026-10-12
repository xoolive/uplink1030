# /// script
# requires-python = ">=3.11"
# dependencies = ["pyopensky>=2.16", "pandas"]
# ///
"""Query OpenSky around the GQRX capture near Brussels Airport."""

from datetime import datetime, timedelta, timezone
from pathlib import Path

import pandas as pd
from pyopensky.trino import Trino

# GQRX uses UTC in its capture filename. This is corroborated by the file's
# 17:34 Brussels-local mtime (CEST, UTC+2) after a 15:32:54 filename start.
START = datetime(2026, 7, 8, 15, 32, 54, tzinfo=timezone.utc)
DURATION = 8_433_061_888 / (20_000_000 * 8)  # bytes / (samples/s * bytes/sample)
STOP = START + timedelta(seconds=DURATION)
# Broad airport/terminal-area box: west, south, east, north.
BOUNDS = (3.5, 50.4, 5.5, 51.4)
OUT = Path("opensky_20260708_153254")
OUT.mkdir(exist_ok=True)

start_epoch = START.timestamp()
stop_epoch = STOP.timestamp()
hour = int(start_epoch // 3600 * 3600)
trino = Trino()

print(f"UTC interval: {START.isoformat()} – {STOP.isoformat()} ({DURATION:.3f} s)")

states_sql = f"""
SELECT time, icao24, trim(callsign) AS callsign, lat, lon,
       baroaltitude, geoaltitude, velocity, heading, vertrate,
       onground, squawk, lastcontact
FROM state_vectors_data4
WHERE hour = {hour}
  AND time BETWEEN {int(start_epoch)} AND {int(stop_epoch) + 1}
  AND lon BETWEEN {BOUNDS[0]} AND {BOUNDS[2]}
  AND lat BETWEEN {BOUNDS[1]} AND {BOUNDS[3]}
ORDER BY time, icao24
"""
states = trino.query(states_sql, cached=True)
states.to_csv(OUT / "state_vectors.csv", index=False)

icao24 = sorted(states.icao24.dropna().str.lower().unique()) if not states.empty else []
print(f"Nearby state vectors: {len(states)} rows, {len(icao24)} aircraft")

if icao24:
    addresses = ", ".join(f"'{addr}'" for addr in icao24)
    # The current OpenSky Trino deployment stores DF4/5 and DF20/21 together
    # in rollcall_replies_data4. Derive DF from the first five message bits.
    replies_sql = f"""
    SELECT mintime, maxtime, icao24, rawmsg, msgcount, sensors,
           bitwise_right_shift(from_base(substr(rawmsg, 1, 2), 16), 3) AS df,
           flightstatus, downlinkrequest, utilitymsg, altitude, identity
    FROM rollcall_replies_data4
    WHERE hour = {hour}
      AND maxtime >= {start_epoch}
      AND mintime <= {stop_epoch}
      AND lower(icao24) IN ({addresses})
    ORDER BY mintime, icao24
    """
    replies = trino.query(replies_sql, cached=True)
else:
    replies = pd.DataFrame()

replies.to_csv(OUT / "rollcall_replies.csv", index=False)

if not states.empty:
    aircraft = (
        states.sort_values("time")
        .groupby("icao24", as_index=False)
        .agg(
            callsign=("callsign", "last"),
            first_time=("time", "min"),
            last_time=("time", "max"),
            lat=("lat", "last"),
            lon=("lon", "last"),
            baroaltitude=("baroaltitude", "last"),
            onground=("onground", "last"),
            squawk=("squawk", "last"),
        )
    )
else:
    aircraft = states
aircraft.to_csv(OUT / "aircraft.csv", index=False)

print(f"Roll-call/surveillance reply rows: {len(replies)}")
if not replies.empty:
    print("DF counts (rows / represented messages):")
    print(replies.groupby("df").agg(rows=("df", "size"), messages=("msgcount", "sum")))
print(f"Wrote {OUT}/")
