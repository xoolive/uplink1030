pub mod bits;
pub mod crc;
pub mod decode;
pub mod dsp;
pub mod source;

pub use decode::{decode_frame, DecodedUplink, UplinkFormat};
pub use dsp::demodulator::{demodulate_snippet, DemodError};
pub use dsp::detector::{Detection, Detector};
pub use dsp::timing::{
    TimingError, UplinkTiming, DEFAULT_UPLINK_SAMPLE_RATE_HZ, MIN_UPLINK_SAMPLE_RATE_HZ,
};
