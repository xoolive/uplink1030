use std::fs;
use std::path::Path;

use desperado::iqread::IqRead;
use desperado::IqFormat;
use num_complex::Complex32;

const MODES_UPLINK_FREQ_HZ: u32 = 1_030_000_000;
const DEFAULT_SAMPLE_RATE_HZ: u32 = 20_000_000;

#[derive(Debug)]
pub enum IqReadError {
    Io(std::io::Error),
    Desperado(desperado::Error),
    InvalidCf32Length(usize),
}

impl std::fmt::Display for IqReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::Desperado(err) => write!(f, "{err}"),
            Self::InvalidCf32Length(len) => write!(
                f,
                "invalid CF32 byte length {len}; expected a multiple of 8"
            ),
        }
    }
}

impl std::error::Error for IqReadError {}

impl From<std::io::Error> for IqReadError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<desperado::Error> for IqReadError {
    fn from(value: desperado::Error) -> Self {
        Self::Desperado(value)
    }
}

pub fn read_cf32_file(path: impl AsRef<Path>) -> Result<Vec<Complex32>, IqReadError> {
    let len = fs::metadata(path.as_ref())?.len() as usize;
    if len % 8 != 0 {
        return Err(IqReadError::InvalidCf32Length(len));
    }
    let chunk_size = len / 8;
    let mut reader = IqRead::from_file(
        path,
        MODES_UPLINK_FREQ_HZ,
        DEFAULT_SAMPLE_RATE_HZ,
        chunk_size,
        IqFormat::Cf32,
    )?;
    match reader.next() {
        Some(samples) => Ok(samples?),
        None => Ok(Vec::new()),
    }
}

pub fn read_cf32_bytes(bytes: &[u8]) -> Result<Vec<Complex32>, IqReadError> {
    if bytes.len() % 8 != 0 {
        return Err(IqReadError::InvalidCf32Length(bytes.len()));
    }
    let mut out = Vec::with_capacity(bytes.len() / 8);
    for chunk in bytes.chunks_exact(8) {
        let i = f32::from_le_bytes(chunk[0..4].try_into().unwrap());
        let q = f32::from_le_bytes(chunk[4..8].try_into().unwrap());
        out.push(Complex32::new(i, q));
    }
    Ok(out)
}
