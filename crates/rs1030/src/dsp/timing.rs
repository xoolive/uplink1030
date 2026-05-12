pub const BIT_RATE_HZ: u32 = 4_000_000;
pub const MIN_UPLINK_SAMPLE_RATE_HZ: u32 = 8_000_000;
pub const DEFAULT_UPLINK_SAMPLE_RATE_HZ: u32 = 20_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UplinkTiming {
    pub sample_rate_hz: u32,
    pub samples_per_bit: usize,
    pub p1_samples: usize,
    pub p2_offset_samples: usize,
    pub p2_samples: usize,
    pub p6_offset_samples: usize,
    pub p6_sync_samples: usize,
    pub p6_data_offset_samples: usize,
    pub p6_guard_samples: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimingError {
    SampleRateTooLow {
        sample_rate_hz: u32,
        minimum_hz: u32,
    },
    NonIntegerSamplesPerBit {
        sample_rate_hz: u32,
        bit_rate_hz: u32,
    },
}

impl std::fmt::Display for TimingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SampleRateTooLow { sample_rate_hz, minimum_hz } => write!(
                f,
                "uplink sample rate {sample_rate_hz} Hz is too low; minimum is {minimum_hz} Hz"
            ),
            Self::NonIntegerSamplesPerBit { sample_rate_hz, bit_rate_hz } => write!(
                f,
                "sample rate {sample_rate_hz} Hz is not an integer multiple of {bit_rate_hz} Hz bit rate"
            ),
        }
    }
}

impl std::error::Error for TimingError {}

impl Default for UplinkTiming {
    fn default() -> Self {
        Self::from_sample_rate(DEFAULT_UPLINK_SAMPLE_RATE_HZ).expect("20 MS/s is valid")
    }
}

impl UplinkTiming {
    pub fn from_sample_rate(sample_rate_hz: u32) -> Result<Self, TimingError> {
        if sample_rate_hz < MIN_UPLINK_SAMPLE_RATE_HZ {
            return Err(TimingError::SampleRateTooLow {
                sample_rate_hz,
                minimum_hz: MIN_UPLINK_SAMPLE_RATE_HZ,
            });
        }
        if sample_rate_hz % BIT_RATE_HZ != 0 {
            return Err(TimingError::NonIntegerSamplesPerBit {
                sample_rate_hz,
                bit_rate_hz: BIT_RATE_HZ,
            });
        }
        let samples_per_bit = (sample_rate_hz / BIT_RATE_HZ) as usize;
        Ok(Self {
            sample_rate_hz,
            samples_per_bit,
            p1_samples: us_to_samples(sample_rate_hz, 0.8),
            p2_offset_samples: us_to_samples(sample_rate_hz, 2.0),
            p2_samples: us_to_samples(sample_rate_hz, 0.8),
            p6_offset_samples: us_to_samples(sample_rate_hz, 3.5),
            p6_sync_samples: us_to_samples(sample_rate_hz, 1.25),
            p6_data_offset_samples: us_to_samples(sample_rate_hz, 1.25) + 2 * samples_per_bit,
            p6_guard_samples: 2 * samples_per_bit,
        })
    }

    pub fn min_samples_short(&self) -> usize {
        self.p6_offset_samples + self.p6_data_offset_samples + 56 * self.samples_per_bit
    }

    pub fn min_samples_long(&self) -> usize {
        self.p6_offset_samples + self.p6_data_offset_samples + 112 * self.samples_per_bit
    }
}

fn us_to_samples(sample_rate_hz: u32, us: f64) -> usize {
    ((sample_rate_hz as f64 * us * 1e-6).round()) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_minimum_sample_rate() {
        assert!(matches!(
            UplinkTiming::from_sample_rate(6_000_000),
            Err(TimingError::SampleRateTooLow { .. })
        ));
        assert_eq!(
            UplinkTiming::from_sample_rate(8_000_000)
                .unwrap()
                .samples_per_bit,
            2
        );
        assert_eq!(
            UplinkTiming::from_sample_rate(20_000_000)
                .unwrap()
                .samples_per_bit,
            5
        );
    }

    #[test]
    fn rejects_non_integer_samples_per_bit_for_now() {
        assert!(matches!(
            UplinkTiming::from_sample_rate(10_000_000),
            Err(TimingError::NonIntegerSamplesPerBit { .. })
        ));
    }
}
