use num_complex::Complex32;

pub const DEFAULT_SAMPLE_RATE: f64 = 20.0e6;
pub const BIT_RATE: f64 = 4.0e6;
pub const SAMPLES_PER_BIT: usize = 5;
pub const P6_OFFSET_SAMPLES: usize = 70;
pub const P6_SYNC_SAMPLES: usize = 25;
pub const P6_DATA_OFFSET_SAMPLES: usize = P6_SYNC_SAMPLES + 2 * SAMPLES_PER_BIT;
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

/// Demodulate a snippet whose P1 starts at sample 0 and whose P6 starts at sample 70.
pub fn demodulate_snippet(iq: &[Complex32]) -> Result<Vec<u8>, DemodError> {
    let num_bits =
        if iq.len() >= P6_OFFSET_SAMPLES + P6_DATA_OFFSET_SAMPLES + LONG_BITS * SAMPLES_PER_BIT {
            LONG_BITS
        } else {
            SHORT_BITS
        };
    demodulate_from_p6(&iq[P6_OFFSET_SAMPLES..], num_bits)
}

/// DPSK demodulate a detected frame inside a larger buffer.
pub fn demodulate_detection(
    iq: &[Complex32],
    p6_sample: usize,
    num_bits: usize,
) -> Result<Vec<u8>, DemodError> {
    if p6_sample > iq.len() {
        return Err(DemodError::NotEnoughSamples {
            needed: p6_sample + 1,
            got: iq.len(),
        });
    }
    demodulate_from_p6(&iq[p6_sample..], num_bits)
}

/// DPSK demodulate samples starting at the beginning of P6.
pub fn demodulate_from_p6(iq: &[Complex32], num_bits: usize) -> Result<Vec<u8>, DemodError> {
    if num_bits != SHORT_BITS && num_bits != LONG_BITS {
        return Err(DemodError::UnsupportedBitLength(num_bits));
    }
    let required = P6_DATA_OFFSET_SAMPLES + num_bits * SAMPLES_PER_BIT;
    if iq.len() < required {
        return Err(DemodError::NotEnoughSamples {
            needed: required,
            got: iq.len(),
        });
    }

    let mut out = vec![0u8; num_bits / 8];
    for n in 0..num_bits {
        let bit_start = P6_DATA_OFFSET_SAMPLES + n * SAMPLES_PER_BIT;
        let mut sum = Complex32::new(0.0, 0.0);
        for s in 0..SAMPLES_PER_BIT {
            sum += iq[bit_start + s] * iq[bit_start + s - SAMPLES_PER_BIT].conj();
        }
        if sum.re < 0.0 {
            let byte_index = n / 8;
            let shift = 7 - (n % 8);
            out[byte_index] |= 1 << shift;
        }
    }
    Ok(out)
}
