//! Source specification and I/Q reading for uplink1030
//!
//! Supports multiple input sources using URI-style paths:
//!
//! ## File Input (feature: file)
//! - `file:///path/to/file.cf32` - Absolute path
//! - `file://~/recordings/sample.cf32` - Home-relative path
//! - `samples/file.cf32` - Plain path (auto-detected as file://)
//! - `file://path.cf32?sample_rate=8000000` - Override sample rate
//!
//! ## HackRF Device (feature: hackrf)
//! HackRF has three gain stages:
//! - **LNA gain** (0-40 dB, low-noise amplifier, RX path)
//! - **VGA gain** (0-62 dB, variable gain amplifier, RX path)
//! - **TX gain** (combined 0-102 dB when using single `gain` parameter)
//!
//! URI parameters:
//! - `hackrf://` - Default (LNA=16dB, VGA=20dB, auto-distributed gain)
//! - `hackrf://?sample_rate=8M` - Custom sample rate
//! - `hackrf://?gain=50` - Combined gain (auto-distributed to LNA/VGA)
//! - `hackrf://?lna_gain=30&vga_gain=35` - Set LNA and VGA separately
//! - `hackrf://?lna_gain=40` - Set only LNA (VGA default=20)
//! - `hackrf://?vga_gain=62` - Set only VGA (LNA default=16)
//! - `hackrf://?amp=1` - Enable TX amplifier (+14 dB)
//! - `hackrf://?bias_tee=1` - Enable antenna bias tee (powers external LNA)
//! - `hackrf://1?...` - Device index
//!
//! Examples:
//! - `hackrf://` - Default gains
//! - `hackrf://?gain=60` - Combined 60 dB (splits between LNA/VGA)
//! - `hackrf://?lna_gain=40&vga_gain=40` - Maximum: 40+40=80 dB
//! - `hackrf://?lna_gain=35&vga_gain=27&amp=1&bias_tee=1` - Full setup with LNA powerby bias tee
//!
//! ## SoapySDR Device (feature: soapy)
//! - `soapy://driver=rtlsdr` - RTL-SDR device
//! - `soapy://driver=hackrf` - HackRF via Soapy
//! - `soapy://driver=rtlsdr?sample_rate=2.4M` - Custom sample rate
//! - `soapy://driver=rtlsdr?gain=30.5` - RX gain in dB
//! - `soapy://driver=rtlsdr?channel=1` - Channel selection
//!
//! Example: `soapy://driver=rtlsdr?sample_rate=2.4M&gain=20&channel=0`

use std::str::FromStr;

#[cfg(any(feature = "hackrf", feature = "soapy"))]
use futures::stream::StreamExt;
use num_complex::Complex32;
use url::Url;

#[cfg(feature = "hackrf")]
use desperado::hackrf::HackRfConfig;
#[cfg(feature = "soapy")]
use desperado::soapy::SoapyConfig;
#[cfg(any(feature = "hackrf", feature = "soapy"))]
use desperado::DeviceConfig;
#[cfg(any(feature = "hackrf", feature = "soapy"))]
use desperado::IqAsyncSource;

#[allow(dead_code)]
const MODES_UPLINK_FREQ_HZ: u32 = 1_030_000_000;
#[allow(dead_code)]
const DEFAULT_CHUNK_SAMPLES: usize = 8192;

/// Error type for source specification and I/O
#[derive(Debug)]
#[allow(dead_code)]
pub enum SourceError {
    /// Invalid URL syntax
    UrlParse(String),
    /// Unsupported scheme
    UnsupportedScheme(String),
    /// I/O error reading file
    Io(std::io::Error),
    /// Invalid CF32 data (not multiple of 8 bytes)
    InvalidCf32Length(usize),
    /// Device configuration error
    DeviceConfig(String),
    /// Feature not compiled in
    FeatureNotAvailable(String),
    /// Parse error
    ParseError(String),
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UrlParse(e) => write!(f, "Invalid URL: {}", e),
            Self::UnsupportedScheme(s) => write!(f, "Unsupported scheme: {}", s),
            Self::Io(e) => write!(f, "I/O error: {}", e),
            Self::InvalidCf32Length(len) => write!(
                f,
                "Invalid CF32 length {}: expected multiple of 8 bytes",
                len
            ),
            Self::DeviceConfig(e) => write!(f, "Device config error: {}", e),
            Self::FeatureNotAvailable(feat) => {
                write!(
                    f,
                    "Feature not available: {} (compile with --features {})",
                    feat, feat
                )
            }
            Self::ParseError(e) => write!(f, "Parse error: {}", e),
        }
    }
}

impl std::error::Error for SourceError {}

impl From<std::io::Error> for SourceError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<url::ParseError> for SourceError {
    fn from(err: url::ParseError) -> Self {
        Self::UrlParse(err.to_string())
    }
}

/// Source specification for I/Q data
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// Local CF32 file path (supports file://, file://~/, file:/// URIs)
    #[cfg(feature = "file")]
    File { path: String, sample_rate: u32 },
    /// HackRF SDR device
    #[cfg(feature = "hackrf")]
    HackRf {
        sample_rate: u32,
        device: Option<usize>,
        /// Combined gain in dB (0-102), auto-distributed to LNA/VGA
        /// If both lna_gain and vga_gain are specified, this is ignored
        gain: Option<i32>,
        /// LNA gain (0-40 dB), overrides gain if specified
        lna_gain: Option<u32>,
        /// VGA gain (0-62 dB), overrides gain if specified
        vga_gain: Option<u32>,
        /// Enable TX amplifier (+14 dB)
        amp_enable: bool,
        /// Enable antenna bias tee
        bias_tee: bool,
    },
    /// SoapySDR device
    #[cfg(feature = "soapy")]
    Soapy {
        sample_rate: u32,
        args: String,
        channel: usize,
        gain: Option<f64>, // Gain in dB
    },
}

impl FromStr for Source {
    type Err = SourceError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // If it doesn't have a scheme, treat it as a file path
        let uri = if !s.contains("://") {
            format!("file://{}", s)
        } else {
            s.to_string()
        };

        let url = Url::parse(&uri).map_err(|e| SourceError::UrlParse(e.to_string()))?;
        let scheme = url.scheme();

        match scheme {
            #[cfg(feature = "file")]
            "file" => {
                // Parse file path from URL
                let path = if let Some(host) = url.host_str() {
                    // file://~/path or file://path
                    format!("{}{}", host, url.path())
                } else {
                    // file:///absolute/path
                    url.path().to_string()
                };

                // Expand ~ to home directory
                let path = shellexpand::tilde(&path).to_string();

                // Extract sample_rate from query parameters
                let sample_rate: u32 = url
                    .query_pairs()
                    .find_map(|(k, v)| {
                        if k == "sample_rate" || k == "rate" {
                            v.parse().ok()
                        } else {
                            None
                        }
                    })
                    .unwrap_or(20_000_000);

                Ok(Source::File { path, sample_rate })
            }
            #[cfg(not(feature = "file"))]
            "file" => Err(SourceError::FeatureNotAvailable("file".to_string())),
            #[cfg(feature = "hackrf")]
            "hackrf" => {
                let sample_rate = url
                    .query_pairs()
                    .find_map(|(k, v)| {
                        if k == "sample_rate" || k == "rate" {
                            v.parse().ok()
                        } else {
                            None
                        }
                    })
                    .unwrap_or(20_000_000);

                let device =
                    url.host_str()
                        .and_then(|h| if h.is_empty() { None } else { h.parse().ok() });

                let gain = url.query_pairs().find_map(|(k, v)| {
                    if k == "gain" || k == "tx_gain" {
                        v.parse().ok()
                    } else {
                        None
                    }
                });

                let lna_gain = url.query_pairs().find_map(|(k, v)| {
                    if k == "lna_gain" || k == "lna" {
                        v.parse().ok()
                    } else {
                        None
                    }
                });

                let vga_gain = url.query_pairs().find_map(|(k, v)| {
                    if k == "vga_gain" || k == "vga" {
                        v.parse().ok()
                    } else {
                        None
                    }
                });

                let amp_enable = url
                    .query_pairs()
                    .any(|(k, v)| (k == "amp" || k == "amp_enable") && v == "1");

                let bias_tee = url
                    .query_pairs()
                    .any(|(k, v)| (k == "bias_tee" || k == "bias") && v == "1");

                Ok(Source::HackRf {
                    sample_rate,
                    device,
                    gain,
                    lna_gain,
                    vga_gain,
                    amp_enable,
                    bias_tee,
                })
            }
            #[cfg(feature = "soapy")]
            "soapy" => {
                let sample_rate = url
                    .query_pairs()
                    .find_map(|(k, v)| {
                        if k == "sample_rate" || k == "rate" {
                            v.parse().ok()
                        } else {
                            None
                        }
                    })
                    .unwrap_or(20_000_000);

                let channel = url
                    .query_pairs()
                    .find_map(|(k, v)| if k == "channel" { v.parse().ok() } else { None })
                    .unwrap_or(0);

                let gain = url.query_pairs().find_map(|(k, v)| {
                    if k == "gain" || k == "rx_gain" {
                        v.parse().ok()
                    } else {
                        None
                    }
                });

                // Everything after soapy:// is device args
                let args = url.path().trim_start_matches('/').to_string();

                Ok(Source::Soapy {
                    sample_rate,
                    args,
                    channel,
                    gain,
                })
            }
            #[cfg(not(feature = "hackrf"))]
            "hackrf" => Err(SourceError::FeatureNotAvailable("hackrf".to_string())),
            #[cfg(not(feature = "soapy"))]
            "soapy" => Err(SourceError::FeatureNotAvailable("soapy".to_string())),
            other => Err(SourceError::UnsupportedScheme(other.to_string())),
        }
    }
}

impl Source {
    /// Create a Source from a file path (auto-detects file:// if not specified)
    #[allow(dead_code)]
    pub fn from_path(path: &str, _sample_rate: u32) -> Result<Self, SourceError> {
        // If it doesn't have a scheme, prepend file://
        let uri = if !path.contains("://") {
            format!("file://{}", path)
        } else {
            path.to_string()
        };
        uri.parse()
    }

    /// Get the sample rate for this source
    pub fn sample_rate(&self) -> u32 {
        match self {
            #[cfg(feature = "file")]
            Source::File { sample_rate, .. } => *sample_rate,
            #[cfg(feature = "hackrf")]
            Source::HackRf { sample_rate, .. } => *sample_rate,
            #[cfg(feature = "soapy")]
            Source::Soapy { sample_rate, .. } => *sample_rate,
        }
    }

    /// Whether this source is a continuous SDR stream rather than a finite snippet.
    pub fn is_continuous(&self) -> bool {
        match self {
            #[cfg(feature = "file")]
            Source::File { .. } => false,
            #[cfg(feature = "hackrf")]
            Source::HackRf { .. } => true,
            #[cfg(feature = "soapy")]
            Source::Soapy { .. } => true,
        }
    }
}

/// Read CF32 samples from a file
fn read_cf32_file(path: &str) -> Result<Vec<Complex32>, SourceError> {
    let bytes = std::fs::read(path)?;

    if bytes.len() % 8 != 0 {
        return Err(SourceError::InvalidCf32Length(bytes.len()));
    }

    let mut samples = Vec::with_capacity(bytes.len() / 8);
    for chunk in bytes.chunks_exact(8) {
        let i = f32::from_le_bytes(chunk[0..4].try_into().unwrap());
        let q = f32::from_le_bytes(chunk[4..8].try_into().unwrap());
        samples.push(Complex32::new(i, q));
    }
    Ok(samples)
}

/// Load all samples from a finite source.
///
/// This is appropriate for files. For continuous SDR sources, prefer
/// `for_each_sample_chunk` so the caller can process the stream incrementally.
pub async fn load_samples(source: &Source) -> Result<Vec<Complex32>, SourceError> {
    let mut samples = Vec::new();
    for_each_sample_chunk(source, |chunk| {
        samples.extend(chunk);
        Ok(())
    })
    .await?;
    Ok(samples)
}

/// Iterate over sample chunks from any source.
///
/// File sources call the callback once. SDR sources call it once per device chunk
/// and usually run until the process is interrupted.
pub async fn for_each_sample_chunk<F>(source: &Source, mut on_chunk: F) -> Result<(), SourceError>
where
    F: FnMut(Vec<Complex32>) -> Result<(), SourceError>,
{
    match source {
        #[cfg(feature = "file")]
        Source::File { path, .. } => on_chunk(read_cf32_file(path)?),
        #[cfg(feature = "hackrf")]
        Source::HackRf {
            sample_rate,
            device,
            gain,
            lna_gain,
            vga_gain,
            amp_enable,
            bias_tee,
        } => {
            stream_hackrf_samples(
                *sample_rate,
                *device,
                *gain,
                *lna_gain,
                *vga_gain,
                *amp_enable,
                *bias_tee,
                on_chunk,
            )
            .await
        }
        #[cfg(feature = "soapy")]
        Source::Soapy {
            sample_rate,
            args,
            channel,
            gain,
        } => stream_soapy_samples(*sample_rate, args, *channel, *gain, on_chunk).await,
    }
}

/// Stream samples from HackRF device.
#[cfg(feature = "hackrf")]
async fn stream_hackrf_samples<F>(
    sample_rate: u32,
    device_idx: Option<usize>,
    gain: Option<i32>,
    lna_gain: Option<u32>,
    vga_gain: Option<u32>,
    amp_enable: bool,
    bias_tee: bool,
    mut on_chunk: F,
) -> Result<(), SourceError>
where
    F: FnMut(Vec<Complex32>) -> Result<(), SourceError>,
{
    // Determine gain settings
    // Priority: lna_gain + vga_gain > gain > Auto
    let gain = if let (Some(lna), Some(vga)) = (lna_gain, vga_gain) {
        // Both LNA and VGA specified: use Elements
        desperado::Gain::Elements(vec![
            desperado::GainElement {
                name: desperado::GainElementName::Lna,
                value_db: lna as f64,
            },
            desperado::GainElement {
                name: desperado::GainElementName::Vga,
                value_db: vga as f64,
            },
        ])
    } else if let Some(g) = gain {
        // Combined gain: let desperado split it
        desperado::Gain::Manual(g as f64)
    } else if lna_gain.is_some() || vga_gain.is_some() {
        // Only one of LNA or VGA specified: use that, let device handle the other
        let elements: Vec<_> = std::iter::empty()
            .chain(lna_gain.map(|lna| desperado::GainElement {
                name: desperado::GainElementName::Lna,
                value_db: lna as f64,
            }))
            .chain(vga_gain.map(|vga| desperado::GainElement {
                name: desperado::GainElementName::Vga,
                value_db: vga as f64,
            }))
            .collect();
        desperado::Gain::Elements(elements)
    } else {
        desperado::Gain::Auto
    };

    let config = HackRfConfig {
        device_index: device_idx.unwrap_or(0),
        center_freq: MODES_UPLINK_FREQ_HZ as u64,
        sample_rate,
        gain,
        amp_enable,
        bias_tee,
    };

    let device_config = DeviceConfig::HackRf(config);
    let mut source = IqAsyncSource::from_device_config(&device_config)
        .await
        .map_err(|e| SourceError::DeviceConfig(e.to_string()))?;

    while let Some(chunk) = source.next().await {
        let chunk = chunk.map_err(|e| SourceError::DeviceConfig(e.to_string()))?;
        on_chunk(chunk)?;
    }
    Ok(())
}

/// Stream samples from SoapySDR device.
#[cfg(feature = "soapy")]
async fn stream_soapy_samples<F>(
    sample_rate: u32,
    args: &str,
    channel: usize,
    gain: Option<f64>,
    mut on_chunk: F,
) -> Result<(), SourceError>
where
    F: FnMut(Vec<Complex32>) -> Result<(), SourceError>,
{
    let gain = if let Some(g) = gain {
        desperado::Gain::Manual(g)
    } else {
        desperado::Gain::Auto
    };

    let config = SoapyConfig {
        args: args.to_string(),
        center_freq: MODES_UPLINK_FREQ_HZ as f64,
        sample_rate: sample_rate as f64,
        channel,
        gain,
        bias_tee: false,
    };

    let device_config = DeviceConfig::Soapy(config);
    let mut source = IqAsyncSource::from_device_config(&device_config)
        .await
        .map_err(|e| SourceError::DeviceConfig(e.to_string()))?;

    while let Some(chunk) = source.next().await {
        let chunk = chunk.map_err(|e| SourceError::DeviceConfig(e.to_string()))?;
        on_chunk(chunk)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(feature = "file")]
    fn parse_file_uri_absolute() {
        let source = "file:///tmp/test.cf32".parse::<Source>();
        assert!(source.is_ok());
        if let Ok(Source::File { path, sample_rate }) = source {
            assert_eq!(path, "/tmp/test.cf32");
            assert_eq!(sample_rate, 20_000_000);
        }
    }

    #[test]
    #[cfg(feature = "file")]
    fn parse_file_uri_with_sample_rate() {
        let source = "file:///tmp/test.cf32?sample_rate=8000000".parse::<Source>();
        assert!(source.is_ok());
        if let Ok(Source::File { sample_rate, .. }) = source {
            assert_eq!(sample_rate, 8_000_000);
        }
    }

    #[test]
    #[cfg(feature = "hackrf")]
    fn parse_hackrf_uri() {
        let source = "hackrf://?sample_rate=8000000".parse::<Source>();
        assert!(source.is_ok());
    }

    #[test]
    #[cfg(feature = "hackrf")]
    fn parse_hackrf_uri_with_gain() {
        let source = "hackrf://?sample_rate=8000000&gain=20&amp=1".parse::<Source>();
        assert!(source.is_ok());
        if let Ok(Source::HackRf {
            gain, amp_enable, ..
        }) = source
        {
            assert_eq!(gain, Some(20));
            assert!(amp_enable);
        }
    }

    #[test]
    #[cfg(feature = "soapy")]
    fn parse_soapy_uri() {
        let source = "soapy://driver=rtlsdr?sample_rate=8000000".parse::<Source>();
        assert!(source.is_ok());
    }

    #[test]
    #[cfg(feature = "hackrf")]
    fn parse_hackrf_uri_with_separate_gains() {
        let source = "hackrf://?lna_gain=30&vga_gain=40".parse::<Source>();
        assert!(source.is_ok());
        if let Ok(Source::HackRf {
            lna_gain, vga_gain, ..
        }) = source
        {
            assert_eq!(lna_gain, Some(30));
            assert_eq!(vga_gain, Some(40));
        }
    }

    #[test]
    #[cfg(feature = "soapy")]
    fn parse_soapy_uri_with_gain() {
        let source = "soapy://driver=rtlsdr?sample_rate=8000000&gain=30.5".parse::<Source>();
        assert!(source.is_ok());
        if let Ok(Source::Soapy { gain, .. }) = source {
            assert_eq!(gain, Some(30.5));
        }
    }
}
