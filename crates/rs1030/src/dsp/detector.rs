use num_complex::Complex32;
use serde::Serialize;

use super::demodulator::{
    LONG_BITS, P6_DATA_OFFSET_SAMPLES, P6_OFFSET_SAMPLES, P6_SYNC_SAMPLES, SAMPLES_PER_BIT,
    SHORT_BITS,
};

const P1_SAMPLES: usize = 16;
const P2_OFFSET_SAMPLES: usize = 40;
const P2_SAMPLES: usize = 16;
const SHORT_DATA_SAMPLES: usize = SHORT_BITS * SAMPLES_PER_BIT;
const LONG_DATA_SAMPLES: usize = LONG_BITS * SAMPLES_PER_BIT;
// The snippets in this repository end right after the last data chip:
// 70 + 35 + 56*5 = 385 samples for short frames, and 665 for long frames.
// Do not require the optional 0.5 us post-P6 guard for detection.
const MIN_SAMPLES_SHORT: usize = P6_OFFSET_SAMPLES + P6_DATA_OFFSET_SAMPLES + SHORT_DATA_SAMPLES;
const MIN_SAMPLES_LONG: usize = P6_OFFSET_SAMPLES + P6_DATA_OFFSET_SAMPLES + LONG_DATA_SAMPLES;
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
}

impl Default for Detector {
    fn default() -> Self {
        Self::new(3.0, false)
    }
}

impl Detector {
    pub fn new(threshold_db: f32, strict: bool) -> Self {
        Self {
            threshold_ratio: 10.0f32.powf(threshold_db / 20.0),
            strict,
            strict_threshold_ratio: 1.5,
            strict_p2_min_ratio: 0.5,
            strict_p2_max_ratio: 2.0,
        }
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
        while i + MIN_SAMPLES_SHORT <= count {
            let p1_avg = avg_mag(&mag, i, P1_SAMPLES);
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
                let p2_start = i as isize + P2_OFFSET_SAMPLES as isize + off;
                if p2_start >= 0 && (p2_start as usize) + P2_SAMPLES <= count {
                    let p2_avg = avg_mag(&mag, p2_start as usize, P2_SAMPLES);
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

            let gap_start = i + P1_SAMPLES;
            let gap_len = P2_OFFSET_SAMPLES - P2_TOLERANCE as usize - P1_SAMPLES;
            let gap_thresh = thresh.max(p1_avg * 0.3);
            if gap_len > 0
                && gap_start + gap_len <= count
                && !gap_below_threshold(&mag, gap_start, gap_len, gap_thresh)
            {
                i += 1;
                continue;
            }

            let p6_start = i + P6_OFFSET_SAMPLES;
            let p6_avg = avg_mag(&mag, p6_start, P6_SYNC_SAMPLES);
            if p6_avg <= thresh || (self.strict && p6_avg < thresh * self.strict_threshold_ratio) {
                i += 1;
                continue;
            }
            if !has_sync_reversal(buf, p6_start) {
                i += 1;
                continue;
            }

            let mut num_bits = SHORT_BITS;
            if i + MIN_SAMPLES_LONG <= count {
                let short_extent = P6_DATA_OFFSET_SAMPLES + SHORT_DATA_SAMPLES;
                let long_extent = P6_DATA_OFFSET_SAMPLES + LONG_DATA_SAMPLES;
                let tail_len = long_extent - short_extent;
                let tail_probe_len = tail_len.min(50);
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
                MIN_SAMPLES_LONG
            } else {
                MIN_SAMPLES_SHORT
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

fn has_sync_reversal(buf: &[Complex32], p6_start: usize) -> bool {
    let window = SAMPLES_PER_BIT;
    let sync_center = p6_start + P6_SYNC_SAMPLES;
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
    use crate::decode::uplink::decode_frame;
    use crate::dsp::demodulator::demodulate_detection;
    use crate::source::iqread::read_cf32_file;

    #[test]
    fn detects_concatenated_sample_snippets() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let first = read_cf32_file(root.join(
            "samples/20260226_140118.895_sdr_time+local_offset_p1-394868014343_snr-23.5.cf32",
        ))
        .unwrap();
        let second = read_cf32_file(root.join(
            "samples/20260226_144151.486_sdr_time+local_offset_p1-443519782409_snr-24.8.cf32",
        ))
        .unwrap();

        let mut stream = Vec::new();
        stream.extend(vec![Complex32::new(0.0, 0.0); 80]);
        stream.extend_from_slice(&first);
        stream.extend(vec![Complex32::new(0.0, 0.0); 120]);
        stream.extend_from_slice(&second);
        stream.extend(vec![Complex32::new(0.0, 0.0); 80]);

        let detections = Detector::default().detect(&stream, 0);
        assert_eq!(detections.len(), 2);
        assert_eq!(detections[0].num_bits, SHORT_BITS);
        assert_eq!(detections[1].num_bits, LONG_BITS);

        let frame0 =
            demodulate_detection(&stream, detections[0].p6_sample, detections[0].num_bits).unwrap();
        let frame1 =
            demodulate_detection(&stream, detections[1].p6_sample, detections[1].num_bits).unwrap();
        assert_eq!(decode_frame(&frame0).unwrap().uf, 0);
        assert_eq!(decode_frame(&frame1).unwrap().uf, 16);
    }
}
