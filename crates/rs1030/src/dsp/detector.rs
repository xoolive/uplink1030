use num_complex::Complex32;
use serde::Serialize;

use super::demodulator::{LONG_BITS, SHORT_BITS};
use super::timing::UplinkTiming;

const P2_TOLERANCE: isize = 2;
const P6_SYNC_TOLERANCE: isize = 1;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Detection {
    pub p1_sample: usize,
    pub p6_sample: usize,
    pub num_bits: usize,
    pub signal_power: f32,
    pub noise_mag: f32,
    pub threshold_mag: f32,
}

#[derive(Debug, Clone)]
pub struct Detector {
    threshold_ratio: f32,
    strict: bool,
    strict_threshold_ratio: f32,
    strict_p2_min_ratio: f32,
    strict_p2_max_ratio: f32,
    timing: UplinkTiming,
}

impl Default for Detector {
    fn default() -> Self {
        Self::new(3.0, false)
    }
}

impl Detector {
    pub fn new(threshold_db: f32, strict: bool) -> Self {
        Self::with_timing(threshold_db, strict, UplinkTiming::default())
    }

    pub fn with_timing(threshold_db: f32, strict: bool, timing: UplinkTiming) -> Self {
        Self {
            threshold_ratio: 10.0f32.powf(threshold_db / 20.0),
            strict,
            strict_threshold_ratio: 1.5,
            strict_p2_min_ratio: 0.5,
            strict_p2_max_ratio: 2.0,
            timing,
        }
    }

    pub fn timing(&self) -> UplinkTiming {
        self.timing
    }

    pub fn detect(&self, buf: &[Complex32], base_idx: usize) -> Vec<Detection> {
        let count = buf.len();
        if count == 0 {
            return Vec::new();
        }

        let mag: Vec<f32> = buf.iter().copied().map(fast_mag).collect();
        // Estimate noise from the quiet lower tail. The snippets are tightly cut
        // around P6, so a global-mean split can overestimate noise because most
        // samples belong to the interrogation. A lower-tail estimate works for
        // both snippets and longer streams with silence/noise gaps.
        let mut sorted_mag = mag.clone();
        sorted_mag.sort_by(|a, b| a.total_cmp(b));
        let noise_count = (sorted_mag.len() / 5).max(1);
        let mut noise = sorted_mag[..noise_count].iter().sum::<f32>() / noise_count as f32;
        if noise < 1e-6 {
            noise = 1e-6;
        }
        let thresh = noise * self.threshold_ratio;

        let mut out = Vec::new();
        let mut i = 0usize;
        while i + self.timing.min_samples_short() <= count {
            let p1_avg = avg_mag(&mag, i, self.timing.p1_samples);
            let rising_edge =
                p1_avg > thresh && (i == 0 || (mag[i] > thresh && mag[i - 1] <= thresh));
            if !rising_edge {
                i += 1;
                continue;
            }

            if self.strict && p1_avg < thresh * self.strict_threshold_ratio {
                i += 1;
                continue;
            }

            let mut p2_ok = false;
            let mut p2_best = 0.0f32;
            for off in -P2_TOLERANCE..=P2_TOLERANCE {
                let p2_start = i as isize + self.timing.p2_offset_samples as isize + off;
                if p2_start >= 0 && (p2_start as usize) + self.timing.p2_samples <= count {
                    let p2_avg = avg_mag(&mag, p2_start as usize, self.timing.p2_samples);
                    if p2_avg > thresh {
                        p2_ok = true;
                        p2_best = p2_best.max(p2_avg);
                    }
                }
            }
            if !p2_ok {
                i += 1;
                continue;
            }
            if self.strict {
                let ratio = p2_best / (p1_avg + 1e-6);
                if ratio < self.strict_p2_min_ratio || ratio > self.strict_p2_max_ratio {
                    i += 1;
                    continue;
                }
            }

            let gap_start = i + self.timing.p1_samples;
            let gap_len = self
                .timing
                .p2_offset_samples
                .saturating_sub(P2_TOLERANCE as usize)
                .saturating_sub(self.timing.p1_samples);
            let gap_thresh = thresh.max(p1_avg * 0.3);
            if gap_len > 0
                && gap_start + gap_len <= count
                && !gap_below_threshold(&mag, gap_start, gap_len, gap_thresh)
            {
                i += 1;
                continue;
            }

            let p6_start = i + self.timing.p6_offset_samples;
            let p6_avg = avg_mag(&mag, p6_start, self.timing.p6_sync_samples);
            if p6_avg <= thresh || (self.strict && p6_avg < thresh * self.strict_threshold_ratio) {
                i += 1;
                continue;
            }
            if !has_sync_reversal(buf, p6_start, &self.timing) {
                i += 1;
                continue;
            }

            let mut num_bits = SHORT_BITS;
            if i + self.timing.min_samples_long() <= count {
                let short_extent =
                    self.timing.p6_data_offset_samples + SHORT_BITS * self.timing.samples_per_bit;
                let long_extent =
                    self.timing.p6_data_offset_samples + LONG_BITS * self.timing.samples_per_bit;
                let tail_len = long_extent - short_extent;
                let tail_probe_len = tail_len.min(10 * self.timing.samples_per_bit);
                let tail_start = p6_start + short_extent;
                if tail_start + tail_probe_len <= count {
                    let tail_avg = avg_mag(&mag, tail_start, tail_probe_len);
                    if tail_avg > thresh {
                        num_bits = LONG_BITS;
                    }
                }
            }

            out.push(Detection {
                p1_sample: base_idx + i,
                p6_sample: base_idx + p6_start,
                num_bits,
                signal_power: p1_avg * p1_avg,
                noise_mag: noise,
                threshold_mag: thresh,
            });

            i += if num_bits == LONG_BITS {
                self.timing.min_samples_long()
            } else {
                self.timing.min_samples_short()
            };
        }

        out
    }
}

fn fast_mag(c: Complex32) -> f32 {
    let re = c.re.abs();
    let im = c.im.abs();
    re.max(im) + 0.4 * re.min(im)
}

fn avg_mag(mag: &[f32], start: usize, len: usize) -> f32 {
    mag[start..start + len].iter().sum::<f32>() / len as f32
}

fn gap_below_threshold(mag: &[f32], start: usize, len: usize, thresh: f32) -> bool {
    avg_mag(mag, start, len) < thresh
}

fn has_sync_reversal(buf: &[Complex32], p6_start: usize, timing: &UplinkTiming) -> bool {
    let window = timing.samples_per_bit;
    let sync_center = p6_start + timing.p6_sync_samples;
    for off in -P6_SYNC_TOLERANCE..=P6_SYNC_TOLERANCE {
        let sync_idx = sync_center as isize + off;
        let pre_start = sync_idx - 2 * window as isize;
        let post_start = sync_idx;
        if pre_start < 0 || post_start < 0 || post_start as usize + window > buf.len() {
            continue;
        }
        let mut pre = Complex32::new(0.0, 0.0);
        let mut post = Complex32::new(0.0, 0.0);
        for s in 0..window {
            pre += buf[pre_start as usize + s];
            post += buf[post_start as usize + s];
        }
        if pre.norm() < 1e-6 || post.norm() < 1e-6 {
            continue;
        }
        if (pre * post.conj()).re < 0.0 {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use num_complex::Complex32;

    use super::*;

    #[test]
    fn detector_struct_is_constructable() {
        let detector = Detector::default();
        assert!(detector.timing().sample_rate_hz > 0);
    }

    #[test]
    fn detector_rejects_empty_buffer() {
        let buf: Vec<Complex32> = Vec::new();
        let detector = Detector::default();
        let detections = detector.detect(&buf, 0);
        assert!(detections.is_empty());
    }
}
