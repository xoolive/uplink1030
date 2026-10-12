//! Bounded streaming rational rate conversion for complex IQ.
//!
//! An 81-tap Blackman-windowed sinc rejects aliases before downsampling.
//! Integer phase progression avoids long-recording clock drift. The centered
//! kernel waits for 40 future input samples: output index n is centered at input
//! coordinate n * input_rate / output_rate, not a delayed origin. Missing endpoint samples are
//! zero padded only at the start and explicit EOF.
use num_complex::Complex32;

const TAPS: usize = 81;
const HALF: usize = TAPS / 2;
const LANES: usize = 8;

pub struct RationalResampler {
    up: usize,
    down: usize,
    position: usize,
    received: usize,
    produced: usize,
    base: usize,
    buffer: Vec<Complex32>,
    kernels: Vec<[f32; TAPS]>,
    finished: bool,
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

impl RationalResampler {
    pub fn new(input_rate: u32, output_rate: u32) -> Result<Self, String> {
        if input_rate == 0 || output_rate == 0 {
            return Err("resampling rates must be positive".into());
        }
        if input_rate as u64 > 4 * output_rate as u64 || output_rate as u64 > 4 * input_rate as u64
        {
            return Err("single-stage rate conversion supports ratios from 1/4 to 4; use a multistage converter for larger ratios".into());
        }
        let divisor = gcd(input_rate, output_rate);
        let up = (output_rate / divisor) as usize;
        let down = (input_rate / divisor) as usize;
        if up > 4096 {
            return Err("rate ratio requires more than 4096 rational phases".into());
        }
        let cutoff = 0.45 * (output_rate as f64 / input_rate as f64).min(1.);
        let kernels = (0..up)
            .map(|phase| {
                let fraction = phase as f64 / up as f64;
                let mut kernel = [0.; TAPS];
                let mut sum = 0.;
                for (i, value) in kernel.iter_mut().enumerate() {
                    let x = i as f64 - HALF as f64 - fraction;
                    let z = 2. * cutoff * x;
                    let sinc = if z.abs() < 1e-12 {
                        1.
                    } else {
                        (std::f64::consts::PI * z).sin() / (std::f64::consts::PI * z)
                    };
                    let angle = std::f64::consts::TAU * i as f64 / (TAPS - 1) as f64;
                    let window = 0.42 - 0.5 * angle.cos() + 0.08 * (2. * angle).cos();
                    let weight = 2. * cutoff * sinc * window;
                    *value = weight as f32;
                    sum += weight;
                }
                for value in &mut kernel {
                    *value = (*value as f64 / sum) as f32;
                }
                kernel
            })
            .collect();
        Ok(Self {
            up,
            down,
            position: 0,
            received: 0,
            produced: 0,
            base: 0,
            buffer: vec![],
            kernels,
            finished: false,
        })
    }

    /// `final_chunk` declares EOF and flushes the bounded lookahead. Exactly
    /// ceil(N*out/in) samples are produced, each with a center before input EOF.
    /// Further input after EOF is a programming error.
    pub fn push(&mut self, input: &[Complex32], final_chunk: bool) -> Vec<Complex32> {
        assert!(!self.finished, "input after resampler EOF");
        self.buffer.extend_from_slice(input);
        self.received += input.len();
        let target =
            ((self.received as u128 * self.up as u128).div_ceil(self.down as u128)) as usize;
        let mut output = Vec::with_capacity(target.saturating_sub(self.produced));
        while self.produced < target {
            let center = self.position / self.up;
            if !final_chunk && center + HALF >= self.received {
                break;
            }
            let phase = self.position % self.up;
            let coefficients = &self.kernels[phase];
            let window = center
                .checked_sub(HALF)
                .and_then(|s| s.checked_sub(self.base))
                .and_then(|s| self.buffer.get(s..s + TAPS));
            let value = if let Some(window) = window {
                // Independent partial sums expose SIMD opportunities without
                // unsafe code or a dependency on CPU-specific instructions.
                let mut sums = [Complex32::default(); LANES];
                for (samples, weights) in window
                    .chunks_exact(LANES)
                    .zip(coefficients.chunks_exact(LANES))
                {
                    for lane in 0..LANES {
                        sums[lane] += samples[lane] * weights[lane];
                    }
                }
                sums.into_iter().sum::<Complex32>() + window[TAPS - 1] * coefficients[TAPS - 1]
            } else {
                let mut value = Complex32::default();
                for (tap, weight) in coefficients.iter().enumerate() {
                    if let Some(index) = (center + tap)
                        .checked_sub(HALF)
                        .and_then(|i| i.checked_sub(self.base))
                    {
                        if let Some(sample) = self.buffer.get(index) {
                            value += *sample * *weight;
                        }
                    }
                }
                value
            };
            output.push(value);
            self.position += self.down;
            self.produced += 1;
        }
        let keep_from = (self.position / self.up)
            .saturating_sub(HALF)
            .min(self.received);
        let consume = keep_from.saturating_sub(self.base).min(self.buffer.len());
        self.buffer.drain(..consume);
        self.base += consume;
        self.finished = final_chunk;
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tone(n: usize, rate: u32, hz: f32) -> Vec<Complex32> {
        (0..n)
            .map(|i| Complex32::from_polar(1., std::f32::consts::TAU * hz * i as f32 / rate as f32))
            .collect()
    }
    #[test]
    fn count_phase_and_chunks_are_invariant() {
        for (input, output) in [
            (10_000_000, 8_000_000),
            (16_000_000, 12_000_000),
            (16_000_000, 10_000_000),
            (10_000_000, 12_000_000),
        ] {
            let data = tone(10007, input, 1_000_000.);
            let reference = RationalResampler::new(input, output)
                .unwrap()
                .push(&data, true);
            assert_eq!(
                reference.len(),
                (data.len() as u64 * output as u64).div_ceil(input as u64) as usize
            );
            for size in [1, 13, 137, 65536] {
                let mut r = RationalResampler::new(input, output).unwrap();
                let mut result = vec![];
                for chunk in data.chunks(size) {
                    result.extend(r.push(chunk, false));
                    assert!(r.buffer.len() <= TAPS + 8);
                }
                result.extend(r.push(&[], true));
                assert_eq!(reference, result);
            }
        }
    }
    #[test]
    fn passband_and_alias_rejection() {
        for hz in [2_000_000., 6_000_000.] {
            let signal = tone(20000, 16_000_000, hz);
            let result = RationalResampler::new(16_000_000, 8_000_000)
                .unwrap()
                .push(&signal, true);
            let rms = (result[100..result.len() - 100]
                .iter()
                .map(Complex32::norm_sqr)
                .sum::<f32>()
                / (result.len() - 200) as f32)
                .sqrt();
            if hz == 2_000_000. {
                assert!((rms - 1.).abs() < 0.002);
            } else {
                assert!(rms < 0.001, "alias RMS {rms}");
            }
        }
    }
    #[test]
    fn centered_impulse_has_no_unreported_group_delay() {
        let mut data = vec![Complex32::default(); 1000];
        data[500] = Complex32::new(1., 0.);
        let result = RationalResampler::new(10_000_000, 8_000_000)
            .unwrap()
            .push(&data, true);
        let peak = result
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.norm_sqr().total_cmp(&b.1.norm_sqr()))
            .unwrap()
            .0;
        assert_eq!(peak, 400);
        assert_eq!(
            RationalResampler::new(10_000_000, 8_000_000)
                .unwrap()
                .push(&[], true)
                .len(),
            0
        );
        assert!(RationalResampler::new(0, 8_000_000).is_err());
        assert!(RationalResampler::new(100_000_000, 8_000_000).is_err());
    }
}
