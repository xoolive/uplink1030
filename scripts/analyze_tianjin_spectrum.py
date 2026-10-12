# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "zstandard"]
# ///
"""Average and peak-hold spectrum for the compressed Tianjin CF32 capture."""
from pathlib import Path
import numpy as np
import zstandard as zstd

PATH = Path.home() / "Documents/data/samples/decode_1030/gqrx_20260730_054541_1030000000_20000000_fc.raw.zst"
FS = 20_000_000.0
FC = 1_030_000_000.0
NFFT = 65536
READ_BYTES = NFFT * 8
window = np.hanning(NFFT).astype(np.float32)
power_sum = np.zeros(NFFT, np.float64)
peak = np.zeros(NFFT, np.float64)
count = 0

with PATH.open("rb") as compressed, zstd.ZstdDecompressor().stream_reader(compressed) as reader:
    while True:
        raw = reader.read(READ_BYTES)
        if not raw:
            break
        if len(raw) < READ_BYTES:
            break
        f = np.frombuffer(raw, dtype="<f4")
        iq = f[0::2] + 1j * f[1::2]
        spec = np.fft.fftshift(np.fft.fft(iq * window))
        p = np.abs(spec) ** 2
        power_sum += p
        peak = np.maximum(peak, p)
        count += 1

freq = FC + np.fft.fftshift(np.fft.fftfreq(NFFT, 1 / FS))
avg_db = 10 * np.log10(power_sum / count + 1e-30)
peak_db = 10 * np.log10(peak + 1e-30)
noise = np.median(avg_db[(abs(freq - FC) > 0.2e6)])

# Aggregate 25 kHz bins to describe persistent spectral features.
bin_hz = 25_000
indices = ((freq - (FC - FS / 2)) // bin_hz).astype(int)
rows = []
for idx in np.unique(indices):
    mask = indices == idx
    rows.append((float(freq[mask].mean()), float(avg_db[mask].mean()), float(peak_db[mask].max())))
rows.sort(key=lambda x: x[1], reverse=True)
print(f"FFT windows: {count}; average noise reference: {noise:.2f} dB")
print("Top 25 kHz bins (frequency MHz, average dB over reference, peak dB over reference):")
for hz, avg, pk in rows[:30]:
    print(f"{hz/1e6:10.5f} {avg-noise:8.2f} {pk-noise:8.2f}")
print("\nBins from 1038 to 1040 MHz:")
for hz, avg, pk in rows:
    if 1038e6 <= hz <= 1040e6 and avg - noise > 1:
        print(f"{hz/1e6:10.5f} {avg-noise:8.2f} {pk-noise:8.2f}")

np.savez_compressed("opensky_20260730_054541/spectrum.npz", frequency_hz=freq, average_db=avg_db, peak_db=peak_db)
