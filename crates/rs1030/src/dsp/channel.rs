//! Stateful baseband translation and sixth-order Butterworth channel filter.
//! Frequencies are in Hz; offset = desired RF frequency minus recording center.
use num_complex::{Complex32, Complex64};

#[derive(Clone)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: Complex64,
    z2: Complex64,
}
impl Biquad {
    fn lowpass(rate: f64, cutoff: f64, q: f64) -> Self {
        let w = std::f64::consts::TAU * cutoff / rate;
        let alpha = w.sin() / (2. * q);
        let a0 = 1. + alpha;
        Self {
            b0: (1. - w.cos()) / (2. * a0),
            b1: (1. - w.cos()) / a0,
            b2: (1. - w.cos()) / (2. * a0),
            a1: -2. * w.cos() / a0,
            a2: (1. - alpha) / a0,
            z1: Complex64::default(),
            z2: Complex64::default(),
        }
    }
    fn apply(&mut self, x: Complex64) -> Complex64 {
        let y = x * self.b0 + self.z1;
        self.z1 = x * self.b1 - y * self.a1 + self.z2;
        self.z2 = x * self.b2 - y * self.a2;
        y
    }
}

pub struct ChannelFilter {
    nco: Complex64,
    step: Complex64,
    count: usize,
    sections: [Biquad; 3],
    remove_dc: bool,
    translate: bool,
}
impl ChannelFilter {
    pub fn new(rate: u32, offset: f64, cutoff: f64, remove_dc: bool) -> Result<Self, String> {
        let nyquist = rate as f64 / 2.;
        if !offset.is_finite()
            || !cutoff.is_finite()
            || cutoff <= 0.
            || cutoff >= nyquist
            || offset.abs() + cutoff > nyquist
        {
            return Err("1030 MHz channel/filter extends outside the sampled bandwidth; check center frequency, sample rate and cutoff".into());
        }
        // Exactly three sections: expose that bound so the compiler can
        // unroll the per-sample cascade without a dynamic Vec loop.
        let sections = std::array::from_fn(|i| {
            let k = 2 - i;
            let q = 1. / (2. * (((2 * k + 1) as f64) * std::f64::consts::PI / 12.).sin());
            Biquad::lowpass(rate as f64, cutoff, q)
        });
        Ok(Self {
            nco: Complex64::new(1., 0.),
            step: Complex64::from_polar(1., -std::f64::consts::TAU * offset / rate as f64),
            count: 0,
            sections,
            remove_dc,
            translate: offset != 0.,
        })
    }
    pub fn apply(&mut self, iq: &mut [Complex32]) {
        if iq.is_empty() {
            return;
        }
        let mean = if self.remove_dc {
            iq.iter().fold(Complex64::default(), |a, x| {
                a + Complex64::new(x.re as f64, x.im as f64)
            }) / iq.len() as f64
        } else {
            Complex64::default()
        };
        for sample in iq {
            let mut x = Complex64::new(sample.re as f64, sample.im as f64) - mean;
            // On-center recordings need no oscillator multiplication/update.
            if self.translate {
                x *= self.nco;
                self.nco *= self.step;
                self.count += 1;
                if self.count.is_multiple_of(4096) {
                    self.nco /= self.nco.norm();
                }
            }
            for section in &mut self.sections {
                x = section.apply(x);
            }
            *sample = Complex32::new(x.re as f32, x.im as f32);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dc_and_bandwidth_checks() {
        let mut f = ChannelFilter::new(16_000_000, -2_000_000., 3_500_000., true).unwrap();
        let mut x = vec![Complex32::new(0.2, -0.3); 1000];
        f.apply(&mut x);
        assert!(x.iter().all(|v| v.norm() < 1e-7));
        assert!(ChannelFilter::new(8_000_000, -2_000_000., 3_500_000., true).is_err());
    }
    #[test]
    fn zero_offset_bypass_matches_identity_oscillator() {
        for dc in [false, true] {
            let mut x: Vec<_> = (0..10000)
                .map(|n| Complex32::new((n as f32 * 0.13).sin(), (n as f32 * 0.07).cos()))
                .collect();
            let mut y = x.clone();
            let mut bypass = ChannelFilter::new(16_000_000, 0., 3_500_000., dc).unwrap();
            let mut reference = ChannelFilter::new(16_000_000, 0., 3_500_000., dc).unwrap();
            reference.translate = true;
            for (a, b) in x.chunks_mut(137).zip(y.chunks_mut(137)) {
                bypass.apply(a);
                reference.apply(b);
            }
            assert_eq!(x, y);
        }
    }

    #[test]
    fn translation_and_filter_are_chunk_invariant() {
        let mut x: Vec<_> = (0..10000)
            .map(|n| Complex32::from_polar(1., -std::f32::consts::TAU * n as f32 / 8.))
            .collect();
        let mut y = x.clone();
        ChannelFilter::new(16_000_000, -2_000_000., 3_500_000., false)
            .unwrap()
            .apply(&mut x);
        let mut f = ChannelFilter::new(16_000_000, -2_000_000., 3_500_000., false).unwrap();
        for c in y.chunks_mut(137) {
            f.apply(c);
        }
        assert_eq!(x, y);
        assert!((x.last().unwrap().norm() - 1.).abs() < 1e-3);
    }
}
