# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "scipy", "pandas", "zstandard"]
# ///
"""Extract per-pair RF features from the full Tianjin 1039 MHz DME signal."""
from pathlib import Path
import numpy as np
import pandas as pd
import zstandard as zstd
from scipy.ndimage import uniform_filter1d
from scipy.signal import find_peaks, peak_widths

CAPTURE = Path.home() / "Documents/data/samples/decode_1030/gqrx_20260730_054541_1030000000_20000000_fc.raw.zst"
OUT = Path("opensky_20260730_054541/dme_pair_features.csv")
FS = 20_000_000.0
CHUNK_SECONDS = 0.25
N = int(FS * CHUNK_SECONDS)
READ_BYTES = N * 8
rows = []
base_sample = 0
chunk_index = 0

def pulse_frequency(iq: np.ndarray, center: int) -> float:
    """Weighted phase-slope carrier estimate over one strong pulse."""
    z = iq[center - 30 : center + 31]
    amp = np.abs(z)
    keep = np.flatnonzero(amp > 0.25)
    if len(keep) < 12:
        return np.nan
    z = z[keep[0] : keep[-1] + 1]
    phase = np.unwrap(np.angle(z))
    weight = np.abs(z) ** 2
    t = np.arange(len(z), dtype=float) / FS
    design = np.column_stack([t, np.ones(len(t))])
    try:
        slope = np.linalg.solve(
            design.T @ (weight[:, None] * design),
            design.T @ (weight * phase),
        )[0]
    except np.linalg.LinAlgError:
        return np.nan
    return slope / (2 * np.pi)

with CAPTURE.open("rb") as compressed, zstd.ZstdDecompressor().stream_reader(compressed) as reader:
    while True:
        raw = reader.read(READ_BYTES)
        if len(raw) < READ_BYTES:
            break
        floats = np.frombuffer(raw, dtype="<f4")
        iq = floats[::2] + 1j * floats[1::2]
        envelope = np.abs(iq)
        smooth = uniform_filter1d(envelope, size=9, mode="nearest")
        peaks, props = find_peaks(smooth, height=0.15, distance=40, prominence=0.10)
        if len(peaks) > 1:
            spacing_us = np.diff(peaks) / FS * 1e6
            pair_idx = np.flatnonzero(np.abs(spacing_us - 12.0) <= 1.0)
            widths = peak_widths(smooth, peaks, rel_height=0.5)[0] / FS * 1e6
            for idx in pair_idx:
                p1, p2 = int(peaks[idx]), int(peaks[idx + 1])
                if p1 < 35 or p2 + 35 >= len(iq):
                    continue
                f1, f2 = pulse_frequency(iq, p1), pulse_frequency(iq, p2)
                rows.append({
                    "time_s": (base_sample + p1) / FS,
                    "spacing_us": spacing_us[idx],
                    "amplitude_p1": smooth[p1],
                    "amplitude_p2": smooth[p2],
                    "amplitude_mean": (smooth[p1] + smooth[p2]) / 2,
                    "amplitude_ratio": smooth[p2] / smooth[p1],
                    "width_p1_us": widths[idx],
                    "width_p2_us": widths[idx + 1],
                    "carrier_p1_hz": f1,
                    "carrier_p2_hz": f2,
                    "carrier_mean_hz": np.nanmean([f1, f2]),
                    "carrier_disagreement_hz": abs(f2 - f1),
                })
        base_sample += N
        chunk_index += 1
        if chunk_index % 100 == 0:
            print(f"{base_sample / FS:.1f} s: {len(rows)} pairs", flush=True)

features = pd.DataFrame(rows)
features.to_csv(OUT, index=False)
print(f"Wrote {len(features)} pairs over {base_sample / FS:.3f} s to {OUT}")
print(features.describe(percentiles=[.01,.1,.5,.9,.99]).to_string())
