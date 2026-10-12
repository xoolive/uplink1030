# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "pandas", "matplotlib", "scikit-learn"]
# ///
"""Assess whether Tianjin DME pulse pairs can be deinterleaved and ADS-B-associated."""
import json
from pathlib import Path
import numpy as np
import pandas as pd
import matplotlib.pyplot as plt
from sklearn.cluster import HDBSCAN
from sklearn.metrics import adjusted_rand_score
from sklearn.mixture import GaussianMixture
from sklearn.preprocessing import RobustScaler

OUT = Path("opensky_20260730_054541")
features = pd.read_csv(OUT / "dme_pair_features.csv")
quality = features[
    (features.amplitude_mean > 0.5)
    & features.spacing_us.between(11.5, 12.5)
    & features.carrier_p1_hz.between(8.95e6, 9.05e6)
    & features.carrier_p2_hz.between(8.95e6, 9.05e6)
    & (features.carrier_disagreement_hz < 15_000)
].copy()

duration = 31_887_796_888 / (20_000_000 * 8)
rate = len(quality) / duration
f_corr = quality[["carrier_p1_hz", "carrier_p2_hz"]].corr().iloc[0, 1]
f_diff_std = (quality.carrier_p2_hz - quality.carrier_p1_hz).std()
# Under the explicit IID-error model, differencing two measurements gives
# sqrt(2)*sigma and averaging them gives sigma/sqrt(2), hence diff_std/2.
f_single_sigma = f_diff_std / np.sqrt(2)
f_pair_mean_sigma = f_diff_std / 2
f_mean_std = quality.carrier_mean_hz.std()
f_lag1 = quality.carrier_mean_hz.autocorr(1)
amp_corr = quality[["amplitude_p1", "amplitude_p2"]].corr().iloc[0, 1]
amp_lag1 = quality.amplitude_mean.autocorr(1)

# Attempt density-based RF fingerprint clustering without time or ADS-B labels.
# If distinct emitters are resolved, HDBSCAN should find stable dense groups in
# carrier/power/shape space. Test several minimum cluster sizes on a fixed,
# representative subsample.
rng_np = np.random.default_rng(1039)
cluster_sample = quality.iloc[
    rng_np.choice(len(quality), min(50_000, len(quality)), replace=False)
]
cluster_columns = [
    "carrier_mean_hz", "amplitude_mean", "amplitude_ratio",
    "width_p1_us", "width_p2_us",
]
cluster_features = RobustScaler().fit_transform(cluster_sample[cluster_columns])
hdbscan_results = {}
for minimum in [100, 300, 1000]:
    labels = HDBSCAN(
        min_cluster_size=minimum,
        min_samples=50,
        cluster_selection_epsilon=0.1,
        copy=True,
    ).fit_predict(cluster_features)
    assigned = labels >= 0
    hdbscan_results[str(minimum)] = {
        "clusters": int(len(np.unique(labels[assigned]))),
        "assigned_fraction": float(assigned.mean()),
    }

# Benchmark frequency-only separability using the empirically derived pair-mean
# error scale. This is deliberately favorable: two balanced emitters, Gaussian
# errors, and no oscillator drift.
separations = np.array([16, 50, 100, 500, 1000, 3000, 6000, 10000], dtype=float)
benchmark_rows = []
for separation in separations:
    scores = []
    for seed in range(10):
        sim_rng = np.random.default_rng(seed)
        labels_true = np.repeat([0, 1], 2000)
        values = np.r_[
            sim_rng.normal(-separation / 2, f_pair_mean_sigma, 2000),
            sim_rng.normal(separation / 2, f_pair_mean_sigma, 2000),
        ].reshape(-1, 1)
        labels_pred = GaussianMixture(n_components=2, random_state=seed).fit_predict(values)
        scores.append(adjusted_rand_score(labels_true, labels_pred))
    benchmark_rows.append({"separation_hz": separation, "mean_ari": np.mean(scores), "std_ari": np.std(scores)})
benchmark = pd.DataFrame(benchmark_rows)
benchmark.to_csv(OUT / "dme_frequency_separation_benchmark.csv", index=False)

# Predicted line-of-sight Doppler histories from fresh OpenSky state vectors.
# Receiver location remains an approximation; use local ENU geometry including
# altitude and vertical rate, adequate over this regional box.
RECEIVER_LAT, RECEIVER_LON, RECEIVER_ALT = 39.125, 117.36, 3.0
C = 299_792_458.0
FC = 1_039_000_000.0
states = pd.read_csv(OUT / "state_vectors.csv")
lat0 = np.deg2rad(RECEIVER_LAT)
dnorth = np.deg2rad(states.lat - RECEIVER_LAT) * 6_371_000
deast = np.deg2rad(states.lon - RECEIVER_LON) * 6_371_000 * np.cos(lat0)
altitude = states.geoaltitude.fillna(states.baroaltitude)
dup = altitude - RECEIVER_ALT
range_3d = np.sqrt(deast**2 + dnorth**2 + dup**2)
heading = np.deg2rad(states.heading)
veast = states.velocity * np.sin(heading)
vnorth = states.velocity * np.cos(heading)
vup = states.vertrate.fillna(0)
range_rate = (veast * deast + vnorth * dnorth + vup * dup) / range_3d
states["predicted_doppler_hz"] = -FC * range_rate / C
states["range_km"] = range_3d / 1000
states["contact_age_s"] = states.time - states.lastcontact
states["fresh"] = states.contact_age_s.between(0, 15)
states.to_csv(OUT / "dme_adsb_doppler.csv", index=False)

doppler = states.dropna(subset=["predicted_doppler_hz"])
fresh_doppler = doppler[doppler.fresh]
doppler_summary = fresh_doppler.groupby("icao24").agg(
    callsign=("callsign", "last"),
    min_doppler_hz=("predicted_doppler_hz", "min"),
    median_doppler_hz=("predicted_doppler_hz", "median"),
    max_doppler_hz=("predicted_doppler_hz", "max"),
    min_range_km=("range_km", "min"),
).reset_index()
doppler_summary.to_csv(OUT / "dme_aircraft_doppler_summary.csv", index=False)

# Separation between aircraft present at the same state-vector timestamp.
simultaneous_spacings = []
for _, group in fresh_doppler.groupby("time"):
    values = np.sort(group.predicted_doppler_hz.unique())
    if len(values) > 1:
        simultaneous_spacings.extend(np.diff(values))
doppler_spacing = np.asarray(simultaneous_spacings)

metrics = {
    "raw_pairs": int(len(features)),
    "quality_pairs": int(len(quality)),
    "duration_s": duration,
    "aggregate_pairs_per_s": rate,
    "minimum_search_mode_emitters_at_150_pps": int(np.ceil(rate / 150)),
    "minimum_tracking_mode_emitters_at_30_pps": int(np.ceil(rate / 30)),
    "carrier_p1_p2_correlation": f_corr,
    "carrier_single_pulse_iid_error_scale_hz": f_single_sigma,
    "carrier_pair_mean_iid_error_scale_hz": f_pair_mean_sigma,
    "carrier_pair_mean_observed_std_hz": f_mean_std,
    "carrier_lag1_autocorrelation": f_lag1,
    "amplitude_p1_p2_correlation": amp_corr,
    "amplitude_lag1_autocorrelation": amp_lag1,
    "hdbscan_results": hdbscan_results,
    "fresh_aircraft_with_doppler": int(len(doppler_summary)),
    "fresh_aircraft_doppler_min_hz": float(fresh_doppler.predicted_doppler_hz.min()),
    "fresh_aircraft_doppler_max_hz": float(fresh_doppler.predicted_doppler_hz.max()),
    "median_simultaneous_aircraft_doppler_spacing_hz": float(np.median(doppler_spacing)),
}
(OUT / "dme_deinterleaving_metrics.json").write_text(json.dumps(metrics, indent=2) + "\n")

# Diagnostics: RF feature separability and the much narrower ADS-B Doppler domain.
sample = quality.iloc[rng_np.choice(len(quality), min(15000, len(quality)), replace=False)].sort_values("time_s")
fig, axes = plt.subplots(3, 1, figsize=(11, 10), constrained_layout=True)
axes[0].scatter(sample.time_s, (sample.carrier_mean_hz - sample.carrier_mean_hz.median()) / 1000,
                s=4, alpha=.12, color="tab:blue", edgecolors="none")
axes[0].set(ylabel="Pair carrier estimate − median (kHz)", title="Per-pair carrier estimates show measurement scatter, not persistent tracks")
axes[0].grid(alpha=.15)

axes[1].scatter((sample.carrier_p1_hz - 9.02e6)/1000, (sample.carrier_p2_hz - 9.02e6)/1000,
                s=5, alpha=.12, color="tab:blue", edgecolors="none")
lo, hi = -10, 15
axes[1].plot([lo, hi], [lo, hi], color="black", ls="--", lw=1)
axes[1].set(xlim=(lo, hi), ylim=(lo, hi), xlabel="P1 carrier estimate relative to 9.02 MHz (kHz)",
            ylabel="P2 carrier estimate relative to 9.02 MHz (kHz)", title=f"P1/P2 carrier correlation = {f_corr:.2f}")
axes[1].grid(alpha=.15)

for _, group in fresh_doppler.groupby("icao24"):
    axes[2].plot(group.time - group.time.min(), group.predicted_doppler_hz, alpha=.25, lw=.8)
axes[2].axhspan(-f_pair_mean_sigma, f_pair_mean_sigma, color="tab:red", alpha=.10,
                label=f"± pair-mean IID error scale ({f_pair_mean_sigma:.0f} Hz)")
axes[2].set(xlabel="Seconds within each OpenSky trajectory", ylabel="Predicted Doppler (Hz)",
            title="ADS-B-predicted Dopplers are not resolvable with current per-pulse estimates")
axes[2].legend()
axes[2].grid(alpha=.15)
fig.savefig(OUT / "dme_deinterleaving_diagnostics.png", dpi=180)

print(json.dumps(metrics, indent=2))
print(OUT / "dme_deinterleaving_diagnostics.png")
