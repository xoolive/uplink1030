# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "scipy", "matplotlib", "zstandard"]
# ///
"""Extract and illustrate representative 1039 MHz DME pulse pairs."""
from pathlib import Path
import numpy as np
import zstandard as zstd
from scipy.signal import firwin, lfilter, find_peaks
import matplotlib.pyplot as plt

PATH = Path.home() / "Documents/data/samples/decode_1030/gqrx_20260730_054541_1030000000_20000000_fc.raw.zst"
OUT = Path("opensky_20260730_054541")
FS = 20e6
OFFSET = 9.02e6
SECONDS = 0.25
n = int(FS * SECONDS)
with PATH.open("rb") as f, zstd.ZstdDecompressor().stream_reader(f) as reader:
    raw = reader.read(n * 8)
floats = np.frombuffer(raw, dtype="<f4")
iq = floats[::2] + 1j * floats[1::2]
t = np.arange(n)
baseband = iq * np.exp(-2j * np.pi * OFFSET * t / FS)
filtered = lfilter(firwin(129, 0.65e6, fs=FS), 1, baseband)
envelope = np.abs(filtered)
threshold = np.quantile(envelope, 0.995)
peaks, props = find_peaks(envelope, height=threshold, distance=int(2e-6 * FS), prominence=threshold-np.median(envelope))
delta_us = np.diff(peaks) / FS * 1e6
pair_indices = np.flatnonzero(np.abs(delta_us - 12) < 1.5)
pair_starts = peaks[pair_indices]

# Use a broad amplitude range rather than only the strongest examples. Each
# displayed curve is an actual measured envelope, aligned on its first pulse.
strength = envelope[pair_starts] + envelope[peaks[pair_indices + 1]]
order = np.argsort(strength)
selected = pair_starts[order[np.linspace(len(order) // 4, len(order) - 1, 30).astype(int)]]
pre_us, post_us = 5, 22
rel_samples = np.arange(-int(pre_us * 1e-6 * FS), int(post_us * 1e-6 * FS) + 1)
rel_us = rel_samples / FS * 1e6
raw_traces = np.asarray([envelope[start + rel_samples] for start in selected])
# Normalize each observed pair only for comparing pulse shape; retain the raw
# envelopes in the CSV alongside these normalized traces.
traces = raw_traces / raw_traces.max(axis=1, keepdims=True)

# This is the timing quantity we can measure without attributing emissions to
# aircraft: the P1-to-P2 spacing of each X-mode pair. Do not confuse it with
# adjacent aggregate pair arrivals, which interleave many transmitters.
pair_spacings_us = delta_us[pair_indices]
spacing_error_us = pair_spacings_us - 12.0
pair_times_s = pair_starts / FS

fig, axes = plt.subplots(2, 1, figsize=(10, 8), constrained_layout=True)
for trace in traces:
    axes[0].plot(rel_us, trace, color="tab:blue", lw=1.0, alpha=.62)
axes[0].axvline(0, color="black", ls="--", lw=1)
axes[0].axvline(12, color="black", ls="--", lw=1)
axes[0].annotate("P1", (0, 1.02), ha="center")
axes[0].annotate("P2", (12, 1.02), ha="center")
axes[0].set(xlabel="Time from first pulse peak (µs)", ylabel="Normalized envelope", title="Actual 1039 MHz DME X-mode pulse pairs")

# Put measured internal spacing error on x and observation time on y.
# The shaded ±0.5 µs region is the nominal X-mode interrogation tolerance.
axes[1].axvspan(-0.5, 0.5, color="tab:green", alpha=.10, label="±0.5 µs tolerance")
axes[1].axvline(0, color="black", ls="--", lw=1)
axes[1].scatter(spacing_error_us, pair_times_s, s=28, alpha=.32, color="tab:blue", edgecolors="none")
axes[1].set(xlabel="Measured P1–P2 spacing error from 12 µs (µs)", ylabel="Time into extract (s)", title="X-mode internal pulse-pair timing error")
axes[1].set_xlim(-1.5, 1.5)
axes[1].legend()
axes[1].grid(alpha=.15)
fig.savefig(OUT / "dme_1039_pulse_pairs.png", dpi=180)

columns = [rel_us]
header = ["relative_time_us"]
for i, (raw_trace, trace) in enumerate(zip(raw_traces, traces), 1):
    columns.extend([raw_trace, trace])
    header.extend([f"pair_{i}_raw_envelope", f"pair_{i}_normalized"])
np.savetxt(OUT / "dme_1039_example_pairs.csv", np.column_stack(columns), delimiter=",", header=",".join(header), comments="")
np.savetxt(
    OUT / "dme_1039_pair_timing.csv",
    np.column_stack([pair_times_s, pair_spacings_us, spacing_error_us]),
    delimiter=",",
    header="time_into_extract_s,p1_p2_spacing_us,error_from_12us",
    comments="",
)
print(f"Detected {len(pair_starts)} X-mode pairs in {SECONDS:.2f} s ({len(pair_starts)/SECONDS:.0f} pairs/s)")
print(f"Plotted {len(selected)} actual pulse pairs")
print("P1–P2 spacing percentiles (us):", np.percentile(pair_spacings_us, [10, 25, 50, 75, 90]))
print(f"Within ±0.5 us: {np.mean(np.abs(spacing_error_us) <= .5):.1%}")
print(OUT / "dme_1039_pulse_pairs.png")
