"""Plot both IIS3 radar captures on one Traffic/cartes map.

uv run --project ../traffic --no-sync scripts/plot_radar_trajectories.py
"""

from __future__ import annotations

import argparse
import json
import sys
from collections import Counter
from dataclasses import dataclass
from itertools import pairwise
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT.parent / "traffic" / "src"))

# Select the local Traffic source and headless plotting backend first.
# ruff: noqa: E402
import cartopy.feature as cfeature
import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
from cartes.crs import Lambert93
from matplotlib.lines import Line2D
from traffic.core import Flight, Traffic
from traffic.data import airports

import numpy as np
import pandas as pd

CAPTURES = [
    ("105707", 1791284227, 163.9955469375),
    ("110053", 1791284453, 294.121728),
]


@dataclass
class RadarLayer:
    context: Traffic
    queries: Traffic
    trajectories: Traffic | None
    highlights: Traffic | None


def forward_callsigns(reference: Traffic) -> Traffic:
    # One operation per aircraft in this short recording. flight_id also keeps
    # leading fixes whose callsign has not yet been received during iteration.
    reference = reference.assign(flight_id=lambda df: df.icao24)
    return Traffic.from_flights(
        flight.sort_values("timestamp").assign(
            callsign=lambda df: df.callsign.ffill()
        )
        for flight in reference
    )


def merge_windows(times, half_width):
    windows = []
    for time in sorted(times):
        start, stop = time - half_width, time + half_width
        if windows and start <= windows[-1][1]:
            windows[-1][1] = max(stop, windows[-1][1])
        else:
            windows.append([start, stop])
    return windows


def clip_segment(p0, p1, t0, t1, start, stop):
    return np.array(
        [
            p0 + (p1 - p0) * ((start - t0) / (t1 - t0)),
            p0 + (p1 - p0) * ((stop - t0) / (t1 - t0)),
        ]
    )


def radar_queries(folder):
    models = json.loads((folder / "reply_clock_model.json").read_text())
    records = []
    for line in (folder / "native_known_full.jsonl").open():
        event = json.loads(line)
        message = event["message"]
        if (
            message["type"] not in ("uf4", "uf5")
            or message["sd"].get("iis") != 3
        ):
            continue
        model = models[min(int(event["seconds"] // 10), len(models) - 1)]
        offset = model["lag_s"] if model["usable"] else None
        records.append(
            {
                "capture": folder.name,
                "icao24": message["icao24"],
                "flight_id": message["icao24"],
                "callsign": None,
                "uf": message["type"],
                "bds": message.get("bds", ""),
                "raw": message["raw"],
                "iq_seconds": event["seconds"],
                "timestamp": pd.to_datetime(
                    event["timestamp"] + offset, unit="s", utc=True
                )
                if offset is not None
                else pd.NaT,
                "clock_offset_s": offset,
                "clock_support": model["support"],
                "status": "pending" if offset is not None else "clock_unusable",
                "longitude": None,
                "latitude": None,
            }
        )
    return Traffic(pd.DataFrame(records)), [
        m["lag_s"] for m in models if m["usable"]
    ]


def locate_queries(flight, queries, times, xy, valid):
    """Locate query Traffic using short brackets, without extrapolation."""
    requested = queries.data.timestamp.to_numpy(dtype="datetime64[ns]")
    index = np.searchsorted(times, requested, side="right") - 1
    located = (index >= 0) & (index < len(valid))
    if len(valid):
        located &= valid[np.clip(index, 0, len(valid) - 1)]
    else:
        located[:] = False
    points = np.full((len(requested), 2), np.nan)
    left = index[located]
    fraction = (requested[located] - times[left]) / (
        times[left + 1] - times[left]
    )
    points[located] = xy[left] + (xy[left + 1] - xy[left]) * fraction[:, None]
    callsigns = np.full(len(requested), None, dtype=object)
    if "callsign" in flight.data:
        callsigns[located] = flight.data.callsign.iloc[left].to_numpy()
    return queries.assign(
        status=np.where(located, "located", "no_short_position_bracket"),
        longitude=points[:, 0],
        latitude=points[:, 1],
        callsign=callsigns,
    )


def capture_layer(
    positions, folder, start, duration, half_width, max_gap, max_speed
):
    queries, offsets = radar_queries(folder)
    pad = half_width + pd.Timedelta(seconds=max_gap)
    context = positions.between(
        pd.to_datetime(start + min(offsets), unit="s", utc=True) - pad,
        pd.to_datetime(start + duration + max(offsets), unit="s", utc=True)
        + pad,
        strict=False,
    )
    if context is None:
        raise ValueError(f"No positions during {folder.name}")
    grey, orange, associated, seen = [], [], [], set()
    for flight in context:
        icao24 = flight.icao24
        seen.add(icao24)
        flight = flight.cumulative_distance(
            compute_gs=False, compute_track=False
        )
        times = flight.data.timestamp.to_numpy(dtype="datetime64[ns]")
        xy = flight.data[["longitude", "latitude"]].to_numpy()
        seconds = np.diff(times) / np.timedelta64(1, "s")
        valid = (
            (seconds > 0)
            & (seconds <= max_gap)
            & (np.diff(flight.data.cumdist) * 1852 <= max_speed * seconds)
        )
        for first, last in pairwise(
            np.r_[0, np.flatnonzero(~valid) + 1, len(times)]
        ):
            if last - first >= 2:
                segment = flight.between(
                    pd.Timestamp(times[first], tz="UTC"),
                    pd.Timestamp(times[last - 1], tz="UTC"),
                    strict=False,
                )
                grey.append(
                    segment.assign(flight_id=f"{folder.name}:{icao24}:{first}")
                )
        aircraft = queries.query(
            "icao24 == @icao24 and status == 'pending'",
            local_dict={"icao24": icao24},
        )
        if aircraft is None:
            continue
        aircraft = locate_queries(flight, aircraft, times, xy, valid)
        associated.append(aircraft)
        located = aircraft.query("status == 'located'")
        if located is None:
            continue
        windows = merge_windows(
            located.data.timestamp.to_numpy(dtype="datetime64[ns]"),
            half_width.to_timedelta64(),
        )
        for i in np.flatnonzero(valid):
            for j, (start_time, stop_time) in enumerate(windows):
                start_time, stop_time = (
                    max(times[i], start_time),
                    min(times[i + 1], stop_time),
                )
                if start_time < stop_time:
                    points = clip_segment(
                        xy[i],
                        xy[i + 1],
                        times[i],
                        times[i + 1],
                        start_time,
                        stop_time,
                    )
                    identity = f"{folder.name}:{icao24}:{i}:{j}"
                    orange.append(
                        Flight(
                            pd.DataFrame(
                                {
                                    "timestamp": pd.to_datetime(
                                        [start_time, stop_time], utc=True
                                    ),
                                    "longitude": points[:, 0],
                                    "latitude": points[:, 1],
                                    "icao24": icao24,
                                    "callsign": flight.data.callsign.iloc[i]
                                    if "callsign" in flight.data
                                    else None,
                                    "flight_id": identity,
                                }
                            )
                        )
                    )
    remaining = queries.query(
        "icao24 not in @seen or status == 'clock_unusable'",
        local_dict={"seen": seen},
    )
    if remaining is not None:
        associated.append(
            remaining.assign(
                status=lambda df: df.status.replace(
                    {"pending": "no_trajectory_in_capture"}
                )
            )
        )
    return RadarLayer(
        context,
        sum(associated).sort_values("iq_seconds"),
        Traffic.from_flights(grey),
        Traffic.from_flights(orange),
    )


def combine_layers(layers):
    def combine(name):
        parts = [
            getattr(layer, name)
            for layer in layers
            if getattr(layer, name) is not None
        ]
        return sum(parts) if parts else None

    return RadarLayer(
        combine("context"),
        combine("queries"),
        combine("trajectories"),
        combine("highlights"),
    )


def draw_map(ax, region):
    ax.set_extent(region, buffer=0.2)
    ax.set_facecolor("#edf5f8")
    ax.add_feature(cfeature.LAND.with_scale("110m"), facecolor="#faf9f5")
    ax.add_feature(
        cfeature.BORDERS.with_scale("110m"), edgecolor="#aab1b4", linewidth=0.6
    )
    ax.coastlines(resolution="10m", color="#aab1b4", linewidth=0.6)
    grid = ax.gridlines(
        draw_labels=True, linewidth=0.35, color="#b4bcc2", alpha=0.6
    )
    grid.top_labels = grid.right_labels = False


def plot_query_points(ax, queries, **kwargs):
    if queries is None:
        return
    for flight in queries:
        if len(flight.data) == 1:
            flight.at().plot(
                ax,
                text_kw={"s": ""},
                marker=".",
                s=3,
                color=kwargs.get("color", "#e56b16"),
            )
        else:
            flight.plot(ax, linestyle="None", marker=".", **kwargs)


def draw_layer(ax, layer, region, positions):
    draw_map(ax, region)
    if layer.trajectories is not None:
        for flight in layer.trajectories:
            flight.plot(ax, color="#8795a1", linewidth=0.9, alpha=0.65)
    if layer.highlights is not None:
        for flight in layer.highlights:
            flight.plot(ax, color="#e56b16", linewidth=2.5, alpha=0.95)
    for code in ("CDG", "ORY"):
        airports[code].point.plot(
            ax,
            marker="+",
            s=25,
            color="#344b60",
            text_kw={"s": code, "fontsize": 8},
        )
    located = layer.queries.query("status == 'located'")
    plot_query_points(ax, located, color="#b94d08", markersize=2, alpha=0.6)
    if located is not None:
        for index, flight in enumerate(
            sorted(located, key=lambda f: len(f.data), reverse=True)[:5]
        ):
            position = flight.sort_values("timestamp").at()
            position.plot(
                ax,
                marker=".",
                s=3,
                color="#b94d08",
                shift={
                    "units": "points",
                    "fig": ax.figure,
                    "x": 12,
                    "y": 10 + 12 * index,
                },
                text_kw={
                    "s": position.callsign
                    if pd.notna(position.callsign)
                    else flight.icao24,
                    "fontsize": 7,
                    "bbox": {
                        "facecolor": "white",
                        "alpha": 0.85,
                        "edgecolor": "none",
                        "pad": 1.5,
                    },
                },
            )
    inset = ax.inset_axes([0.025, 0.025, 0.16, 0.3], projection=Lambert93())
    inset.set_extent(positions, buffer=0.5)
    inset.add_feature(cfeature.LAND.with_scale("110m"), facecolor="#f4f5f4")
    inset.coastlines(resolution="110m", linewidth=0.4, color="#7f8c93")
    for flight in layer.context:
        flight.plot(
            inset, linestyle="None", marker=".", markersize=1, color="#8795a1"
        )
    plot_query_points(inset, located, color="#e56b16", markersize=1)
    inset.set_title("All positions", fontsize=6, pad=2)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--reference",
        type=Path,
        default=Path.home()
        / "gqrx_20261006_110053_1032000000_16000000_fc.jsonl.zst",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=ROOT / "radar_trajectories_20261006/combined",
    )
    parser.add_argument("--half-window", type=float, default=0.5)
    parser.add_argument("--max-position-gap", type=float, default=5.0)
    parser.add_argument("--max-speed-mps", type=float, default=1000.0)
    args = parser.parse_args()
    reference = forward_callsigns(Traffic.from_file(args.reference))
    positions = reference.query("latitude.notnull() and longitude.notnull()")
    region = positions.query("48 < latitude < 50")
    layers = [
        capture_layer(
            positions,
            ROOT / f"opensky_20261006_{tag}",
            start,
            duration,
            pd.Timedelta(seconds=args.half_window),
            args.max_position_gap,
            args.max_speed_mps,
        )
        for tag, start, duration in CAPTURES
    ]
    combined = combine_layers(layers)
    fig, ax = plt.subplots(
        figsize=(12, 9), subplot_kw={"projection": Lambert93()}
    )
    draw_layer(ax, combined, region, positions)
    located = combined.queries.query("status == 'located'")
    ax.set_title(
        "Both radar captures · 6 October 2026\n"
        f"{len(located.data):,}/{len(combined.queries.data):,} "
        "queries positioned",
        fontsize=16,
        pad=15,
    )
    fig.legend(
        handles=[
            Line2D(
                [0], [0], color="#8795a1", label="Downlink trajectory context"
            ),
            Line2D(
                [0],
                [0],
                color="#e56b16",
                lw=3,
                label=f"UF4/UF5, IIS3 · ±{args.half_window:g} s display window",
            ),
        ],
        loc="lower center",
        bbox_to_anchor=(0.5, 0.075),
        ncol=2,
        frameon=False,
    )
    fig.text(
        0.5,
        0.03,
        "10:57:07\u201310:59:51 and 11:00:53\u201311:05:47 UTC; "
        "no bridging the capture gap.\n"
        f"No interpolation across gaps >{args.max_position_gap:g} s. "
        "Clock-corrected candidates, not confirmed replies "
        "or RF pulse durations.",
        ha="center",
        fontsize=9,
    )
    fig.subplots_adjust(left=0.065, right=0.98, top=0.89, bottom=0.15)
    args.output_dir.mkdir(parents=True, exist_ok=True)
    png = args.output_dir / "radar_interrogation_trajectories.png"
    fig.savefig(png, dpi=190, facecolor="white")
    plt.close(fig)
    combined.queries.to_csv(
        args.output_dir / "radar_query_positions.csv", index=False
    )
    summary = {
        "reference": str(args.reference),
        "message_rows": len(reference.data),
        "position_rows": len(positions.data),
        "projection": "Lambert93",
        "query_rows": len(combined.queries.data),
        "located_queries": len(located.data),
        "located_aircraft": located.data.icao24.nunique(),
        "per_capture": [
            {
                "capture": tag,
                "queries": len(layer.queries.data),
                "statuses": dict(Counter(layer.queries.data.status)),
            }
            for (tag, _, _), layer in zip(CAPTURES, layers)
        ],
        "png": str(png),
    }
    (args.output_dir / "summary.json").write_text(
        json.dumps(summary, indent=2) + "\n"
    )
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
