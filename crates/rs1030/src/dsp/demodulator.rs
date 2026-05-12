use num_complex::Complex32;

use super::timing::UplinkTiming;

pub const SHORT_BITS: usize = 56;
pub const LONG_BITS: usize = 112;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DemodError {
    UnsupportedBitLength(usize),
    NotEnoughSamples { needed: usize, got: usize },
}

impl std::fmt::Display for DemodError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedBitLength(bits) => {
                write!(f, "unsupported Mode S uplink length: {bits} bits")
            }
            Self::NotEnoughSamples { needed, got } => {
                write!(f, "not enough samples: need {needed}, got {got}")
            }
        }
    }
}

impl std::error::Error for DemodError {}

/// Demodulate a 20 MS/s snippet whose P1 starts at sample 0.
pub fn demodulate_snippet(iq: &[Complex32]) -> Result<Vec<u8>, DemodError> {
    demodulate_snippet_with_timing(iq, &UplinkTiming::default())
}

pub fn demodulate_snippet_with_timing(
    iq: &[Complex32],
    timing: &UplinkTiming,
) -> Result<Vec<u8>, DemodError> {
    let num_bits = if iq.len() >= timing.min_samples_long() {
        LONG_BITS
    } else {
        SHORT_BITS
    };
    if iq.len() < timing.p6_offset_samples {
        return Err(DemodError::NotEnoughSamples {
            needed: timing.p6_offset_samples,
            got: iq.len(),
        });
    }
    demodulate_from_p6_with_timing(&iq[timing.p6_offset_samples..], num_bits, timing)
}

/// DPSK demodulate a detected frame inside a larger buffer using default 20 MS/s timing.
pub fn demodulate_detection(
    iq: &[Complex32],
    p6_sample: usize,
    num_bits: usize,
) -> Result<Vec<u8>, DemodError> {
    demodulate_detection_with_timing(iq, p6_sample, num_bits, &UplinkTiming::default())
}

pub fn demodulate_detection_with_timing(
    iq: &[Complex32],
    p6_sample: usize,
    num_bits: usize,
    timing: &UplinkTiming,
) -> Result<Vec<u8>, DemodError> {
    if p6_sample > iq.len() {
        return Err(DemodError::NotEnoughSamples {
            needed: p6_sample + 1,
            got: iq.len(),
        });
    }
    demodulate_from_p6_with_timing(&iq[p6_sample..], num_bits, timing)
}

/// DPSK demodulate samples starting at the beginning of P6 using default 20 MS/s timing.
pub fn demodulate_from_p6(iq: &[Complex32], num_bits: usize) -> Result<Vec<u8>, DemodError> {
    demodulate_from_p6_with_timing(iq, num_bits, &UplinkTiming::default())
}

pub fn demodulate_from_p6_with_timing(
    iq: &[Complex32],
    num_bits: usize,
    timing: &UplinkTiming,
) -> Result<Vec<u8>, DemodError> {
    if num_bits != SHORT_BITS && num_bits != LONG_BITS {
        return Err(DemodError::UnsupportedBitLength(num_bits));
    }
    let required = timing.p6_data_offset_samples + num_bits * timing.samples_per_bit;
    if iq.len() < required {
        return Err(DemodError::NotEnoughSamples {
            needed: required,
            got: iq.len(),
        });
    }

    let mut out = vec![0u8; num_bits / 8];
    for n in 0..num_bits {
        let bit_start = timing.p6_data_offset_samples + n * timing.samples_per_bit;
        let mut sum = Complex32::new(0.0, 0.0);
        for s in 0..timing.samples_per_bit {
            sum += iq[bit_start + s] * iq[bit_start + s - timing.samples_per_bit].conj();
        }
        if sum.re < 0.0 {
            let byte_index = n / 8;
            let shift = 7 - (n % 8);
            out[byte_index] |= 1 << shift;
        }
    }
    Ok(out)
}
