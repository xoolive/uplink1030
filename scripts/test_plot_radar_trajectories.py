"""Regression checks for the radar-map adapter, not Traffic library changes."""

import json
from types import SimpleNamespace

import plot_radar_trajectories as radar_plot
import pytest
from cartes.crs import PlateCarree
from plot_radar_trajectories import (
    Lambert93,
    capture_layer,
    clip_segment,
    merge_windows,
    plt,
)
from traffic.core import Flight, Traffic
from traffic.core.flight import Position

import numpy as np
import pandas as pd


@pytest.fixture
def capture(tmp_path):
    (tmp_path / "reply_clock_model.json").write_text(
        json.dumps(
            [
                {"center_s": 5, "lag_s": 0.9, "support": 9, "usable": True},
            ]
        )
    )
    events = [
        {
            "message": {
                "type": "uf4",
                "icao24": "39bda4",
                "sd": {"iis": 3},
                "raw": "20a93040bfa0ad",
            },
            "seconds": i,
            "timestamp": t,
        }
        for i, t in enumerate([100.1, 102.1, 104.1])
    ]
    (tmp_path / "native_known_full.jsonl").write_text(
        "\n".join(map(json.dumps, events))
    )
    return tmp_path


def positions():
    return Traffic(
        pd.DataFrame(
            {
                "icao24": ["39bda4"] * 3,
                "flight_id": ["39bda4"] * 3,
                "callsign": ["TEST"] * 3,
                "timestamp": pd.to_datetime(
                    [100, 102, 104], unit="s", utc=True
                ),
                "longitude": [2.0, 2.002, 8.0],
                "latitude": [49.0, 49.0, 49.0],
            }
        )
    )


def test_traffic_distance_guard_and_capture_edge_interpolation(capture):
    result = capture_layer(
        positions(), capture, 100, 5, pd.Timedelta(seconds=0.5), 5, 1000
    )
    assert isinstance(result.queries, Traffic)
    assert result.queries.data.status.to_list() == [
        "located",
        "no_short_position_bracket",
        "no_short_position_bracket",
    ]
    assert len(list(result.trajectories)) == len(list(result.highlights)) == 1
    assert all(
        isinstance(f, Flight) for f in result.trajectories + result.highlights
    )
    located = result.queries.query("status == 'located'")
    np.testing.assert_allclose(
        located.data[["longitude", "latitude"]], [[2.001, 49.0]]
    )
    assert located.data.callsign.to_list() == ["TEST"]


def test_singleton_positions_are_not_a_usable_trajectory(capture):
    result = capture_layer(
        Traffic(positions().data.iloc[:1]),
        capture,
        100,
        5,
        pd.Timedelta(seconds=0.5),
        5,
        1000,
    )
    assert result.trajectories is None and result.highlights is None
    assert set(result.queries.data.status) == {"no_short_position_bracket"}


def test_unsorted_fixes_are_ordered_by_traffic(capture):
    half_width = pd.Timedelta(seconds=0.5)
    expected = capture_layer(positions(), capture, 100, 5, half_width, 5, 1000)
    actual = capture_layer(
        Traffic(positions().data.iloc[::-1]),
        capture,
        100,
        5,
        half_width,
        5,
        1000,
    )
    pd.testing.assert_frame_equal(actual.queries.data, expected.queries.data)
    for kind in ("trajectories", "highlights"):
        for a, b in zip(getattr(actual, kind), getattr(expected, kind)):
            pd.testing.assert_frame_equal(a.data, b.data)


def test_cartes_buffered_extent_and_lambert93():
    fig, ax = plt.subplots(subplot_kw={"projection": Lambert93()})
    try:
        ax.set_extent(positions(), buffer=0.2)
        west, east, south, north = ax.get_extent(crs=PlateCarree())
        assert west < 1.85 and east > 8.15
        assert south < 48.85 and north > 49.15
        assert ax.projection.proj4_params["lat_0"] == 46.5
    finally:
        plt.close(fig)


def test_traffic_input_is_not_normalized_or_mutated(capture):
    source = positions()
    before = source.data.copy()
    capture_layer(source, capture, 100, 5, pd.Timedelta(seconds=0.5), 5, 1000)
    pd.testing.assert_frame_equal(source.data, before)
    assert "epoch" not in source.data.columns
    assert "cumdist" not in source.data.columns


def test_native_plotting_wrappers(capture, monkeypatch):
    # No airport download in this regression: the same Point API is exercised.
    monkeypatch.setattr(
        radar_plot,
        "airports",
        {
            code: SimpleNamespace(
                point=Position(
                    {
                        "latitude": 49.0,
                        "longitude": 2.0,
                        "name": code,
                    }
                )
            )
            for code in ("CDG", "ORY")
        },
    )
    source = positions()
    panel = capture_layer(
        source, capture, 100, 5, pd.Timedelta(seconds=0.5), 5, 1000
    )
    fig, ax = plt.subplots(subplot_kw={"projection": Lambert93()})
    try:
        radar_plot.draw_layer(ax, panel, source, source)
        text = [label.get_text() for label in ax.texts]
        assert "CDG" in text and "ORY" in text and "TEST" in text
        assert "BVA" not in text
        assert ax.lines  # Flight.plot handles lines and marker-only points.
    finally:
        plt.close(fig)


def test_callsigns_are_forward_filled_per_aircraft_without_losing_early_fixes():
    source = Traffic(
        pd.DataFrame(
            {
                "icao24": ["a", "b", "a", "b", "a", "b"],
                "timestamp": pd.to_datetime(range(6), unit="s", utc=True),
                "callsign": [None, None, "ALPHA", "BRAVO", None, None],
            }
        )
    )
    before = source.data.copy()
    result = radar_plot.forward_callsigns(source)
    assert len(result.data) == 6
    assert len(list(result)) == 2
    for flight in result:
        assert pd.isna(flight.data.callsign.iloc[0])
        assert flight.data.callsign.iloc[-1] == (
            "ALPHA" if flight.icao24 == "a" else "BRAVO"
        )
    pd.testing.assert_frame_equal(source.data, before)


def test_query_traffic_retains_uncalibrated_events(capture):
    models = json.loads((capture / "reply_clock_model.json").read_text())
    models[0]["usable"] = False
    (capture / "reply_clock_model.json").write_text(json.dumps(models))
    queries, offsets = radar_plot.radar_queries(capture)
    assert isinstance(queries, Traffic) and len(queries.data) == 3
    assert queries.data.timestamp.isna().all()
    assert set(queries.data.status) == {"clock_unusable"}
    assert not offsets


def test_combining_captures_does_not_connect_the_gap(capture):
    later = capture / "later"
    later.mkdir()
    (later / "reply_clock_model.json").write_text(
        (capture / "reply_clock_model.json").read_text()
    )
    events = [
        json.loads(line)
        for line in (capture / "native_known_full.jsonl")
        .read_text()
        .splitlines()
    ]
    for event in events:
        event["timestamp"] += 100
    (later / "native_known_full.jsonl").write_text(
        "\n".join(map(json.dumps, events))
    )
    args = (pd.Timedelta(seconds=0.5), 5, 1000)
    first = capture_layer(positions(), capture, 100, 5, *args)
    second = capture_layer(
        positions().assign(
            timestamp=lambda df: df.timestamp + pd.Timedelta(seconds=100)
        ),
        later,
        200,
        5,
        *args,
    )
    merged = radar_plot.combine_layers([first, second])
    assert len(merged.queries.data) == 6
    assert len(list(merged.highlights)) == 2
    assert len(list(merged.trajectories)) == 2
    assert all(
        f.stop - f.start <= pd.Timedelta(seconds=2)
        for f in merged.trajectories + merged.highlights
    )


def test_highlight_windows_and_clipped_endpoints():
    np.testing.assert_allclose(
        merge_windows([1, 1.25, 5], 0.5), [[0.5, 1.75], [4.5, 5.5]]
    )
    assert merge_windows([], 0.5) == []
    np.testing.assert_allclose(
        clip_segment(np.array([0.0, 0.0]), np.array([1.0, 2.0]), 0, 4, 1, 3),
        [[0.25, 0.5], [0.75, 1.5]],
    )
