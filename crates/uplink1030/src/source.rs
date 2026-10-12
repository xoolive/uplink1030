//! Bounded IQ sources. Device URI configuration is delegated to Desperado.
//! Files (including Zstandard), stdin and TCP accept CF32/CU8/CS8/CS16.
use chrono::NaiveDateTime;
use desperado::IqFormat;
#[cfg(any(
    feature = "hackrf",
    feature = "soapy",
    feature = "airspy",
    feature = "pluto"
))]
use futures::StreamExt;
use num_complex::Complex32;
use std::{io::Read, str::FromStr};
use url::Url;

const CHUNK_SAMPLES: usize = 65_536;
const DEFAULT_RATE: u32 = 20_000_000;
const UPLINK_HZ: u32 = 1_030_000_000;

pub type SourceError = Box<dyn std::error::Error>;

#[derive(Debug)]
enum Input {
    File(String),
    Stdin,
    Tcp(String, u16),
    #[cfg(any(
        feature = "hackrf",
        feature = "soapy",
        feature = "airspy",
        feature = "pluto"
    ))]
    Device(desperado::DeviceConfig),
}

#[derive(Debug)]
pub struct Source {
    input: Input,
    pub sample_rate: u32,
    pub center_frequency: u32,
    pub format: IqFormat,
    pub start_timestamp: Option<f64>,
}

fn parse_rate(s: &str) -> Result<u32, SourceError> {
    Ok(desperado::parse_si_value(s)?)
}

impl FromStr for Source {
    type Err = SourceError;
    fn from_str(spec: &str) -> Result<Self, Self::Err> {
        let mut source = Self {
            input: Input::Stdin,
            sample_rate: DEFAULT_RATE,
            center_frequency: UPLINK_HZ,
            format: IqFormat::Cf32,
            start_timestamp: None,
        };
        if !spec.contains("://") {
            if spec != "-" {
                source.set_file(shellexpand::tilde(spec).to_string());
            }
            return Ok(source);
        }
        // Device parsers also support colon-containing Pluto USB/IP identifiers
        // that are not valid URL authorities. Only use Url for transport sources.
        let scheme = spec.split_once("://").unwrap().0;
        if scheme == "rtlsdr" {
            return Err("RTL-SDR is not supported: uplink reception requires an actual IQ sample rate of at least 8 MS/s, beyond RTL-SDR sample rates".into());
        }
        if !matches!(scheme, "file" | "stdin" | "tcp") {
            #[cfg(any(
                feature = "hackrf",
                feature = "soapy",
                feature = "airspy",
                feature = "pluto"
            ))]
            {
                let query = spec.split_once('?').map_or("", |(_, q)| q);
                for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
                    match key.as_ref() {
                        "rate" | "sample_rate" => source.sample_rate = parse_rate(&value)?,
                        "freq" | "frequency" => source.center_frequency = parse_rate(&value)?,
                        _ => {}
                    }
                }
                let normalized = format!(
                    "{spec}{}freq={}&rate={}",
                    if spec.contains('?') { "&" } else { "?" },
                    source.center_frequency,
                    source.sample_rate
                );
                source.input = Input::Device(normalized.parse()?);
                return Ok(source);
            }
            #[cfg(not(any(
                feature = "hackrf",
                feature = "soapy",
                feature = "airspy",
                feature = "pluto"
            )))]
            return Err(format!(
                "SDR backend '{scheme}' is not enabled; rebuild with --features {scheme}"
            )
            .into());
        }
        let url = Url::parse(spec)?;
        match scheme {
            "file" => {
                let path = if url.host_str().is_none() || url.host_str() == Some("localhost") {
                    url.to_file_path()
                        .map_err(|_| "invalid file URI")?
                        .to_string_lossy()
                        .into_owned()
                } else {
                    percent_encoding::percent_decode_str(&format!(
                        "{}{}",
                        url.host_str().unwrap(),
                        url.path()
                    ))
                    .decode_utf8()?
                    .into_owned()
                };
                source.set_file(shellexpand::tilde(&path).to_string());
            }
            "tcp" => {
                source.input = Input::Tcp(
                    url.host_str()
                        .ok_or("TCP source requires a host")?
                        .to_string(),
                    url.port().ok_or("TCP source requires a port")?,
                )
            }
            "stdin" => {}
            _ => unreachable!(),
        }
        for (key, value) in url.query_pairs() {
            match key.as_ref() {
                "rate" | "sample_rate" => source.sample_rate = parse_rate(&value)?,
                "freq" | "frequency" => source.center_frequency = parse_rate(&value)?,
                "format" => source.format = value.parse()?,
                _ => return Err(format!("unknown IQ source option '{key}'").into()),
            }
        }
        Ok(source)
    }
}

impl Source {
    fn set_file(&mut self, path: String) {
        // GQRX records UTC, center frequency and sample rate in its filename.
        let name = std::path::Path::new(&path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        let parts: Vec<_> = name.split('_').collect();
        if parts.len() >= 6 && parts[0] == "gqrx" && parts[5].starts_with("fc.") {
            if let (Ok(freq), Ok(rate)) = (parts[3].parse::<u32>(), parts[4].parse::<u32>()) {
                self.center_frequency = freq;
                self.sample_rate = rate;
            }
            if let Ok(date) =
                NaiveDateTime::parse_from_str(&format!("{}{}", parts[1], parts[2]), "%Y%m%d%H%M%S")
            {
                self.start_timestamp = Some(date.and_utc().timestamp() as f64);
            }
        }
        self.input = Input::File(path);
    }

    pub fn is_live(&self) -> bool {
        !matches!(self.input, Input::File(_))
    }
    pub fn file_path(&self) -> Option<&str> {
        if let Input::File(p) = &self.input {
            Some(p)
        } else {
            None
        }
    }

    pub fn configure(&mut self, rate: Option<u32>, freq: Option<u32>, format: Option<IqFormat>) {
        if let Some(rate) = rate {
            self.sample_rate = rate;
        }
        if let Some(freq) = freq {
            self.center_frequency = freq;
        }
        if let Some(format) = format {
            self.format = format;
        }
        #[cfg(any(
            feature = "hackrf",
            feature = "soapy",
            feature = "airspy",
            feature = "pluto"
        ))]
        if let Input::Device(config) = &mut self.input {
            match config {
                #[cfg(feature = "hackrf")]
                desperado::DeviceConfig::HackRf(c) => {
                    c.sample_rate = self.sample_rate;
                    c.center_freq = self.center_frequency as u64;
                }
                #[cfg(feature = "soapy")]
                desperado::DeviceConfig::Soapy(c) => {
                    c.sample_rate = self.sample_rate as f64;
                    c.center_freq = self.center_frequency as f64;
                }
                #[cfg(feature = "airspy")]
                desperado::DeviceConfig::Airspy(c) => {
                    c.sample_rate = self.sample_rate;
                    c.center_freq = self.center_frequency;
                }
                #[cfg(feature = "pluto")]
                desperado::DeviceConfig::Pluto(c) => {
                    c.sample_rate = self.sample_rate as i64;
                    c.center_freq = self.center_frequency as i64;
                }
            }
        }
    }
}

fn sample_bytes(format: IqFormat) -> usize {
    match format {
        IqFormat::Cf32 => 8,
        IqFormat::Cs16 => 4,
        IqFormat::Cu8 | IqFormat::Cs8 => 2,
    }
}

fn convert(bytes: &[u8], format: IqFormat) -> Result<Vec<Complex32>, SourceError> {
    let width = sample_bytes(format);
    if !bytes.len().is_multiple_of(width) {
        return Err(format!(
            "incomplete IQ sample: {} bytes, format {:?}",
            bytes.len(),
            format
        )
        .into());
    }
    let mut samples = Vec::with_capacity(bytes.len() / width);
    for b in bytes.chunks_exact(width) {
        let s = match format {
            IqFormat::Cf32 => Complex32::new(
                f32::from_le_bytes(b[..4].try_into().unwrap()),
                f32::from_le_bytes(b[4..].try_into().unwrap()),
            ),
            IqFormat::Cs16 => Complex32::new(
                i16::from_le_bytes(b[..2].try_into().unwrap()) as f32 / 32768.,
                i16::from_le_bytes(b[2..].try_into().unwrap()) as f32 / 32768.,
            ),
            IqFormat::Cs8 => Complex32::new(b[0] as i8 as f32 / 128., b[1] as i8 as f32 / 128.),
            IqFormat::Cu8 => {
                Complex32::new((b[0] as f32 - 127.5) / 128., (b[1] as f32 - 127.5) / 128.)
            }
        };
        if !s.re.is_finite() || !s.im.is_finite() {
            return Err("non-finite IQ sample (check input format)".into());
        }
        samples.push(s);
    }
    Ok(samples)
}

fn read_chunks(
    reader: &mut dyn Read,
    format: IqFormat,
    callback: &mut impl FnMut(Vec<Complex32>) -> Result<bool, SourceError>,
) -> Result<(), SourceError> {
    let mut bytes = vec![0; CHUNK_SAMPLES * sample_bytes(format)];
    loop {
        let mut n = 0;
        while n < bytes.len() {
            match reader.read(&mut bytes[n..]) {
                Ok(0) => break,
                Ok(count) => n += count,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }
        if n == 0 {
            break;
        }
        if !callback(convert(&bytes[..n], format)?)? {
            break;
        }
    }
    Ok(())
}

pub async fn for_each_sample_chunk(
    source: &Source,
    mut callback: impl FnMut(Vec<Complex32>) -> Result<bool, SourceError>,
) -> Result<(), SourceError> {
    match &source.input {
        Input::File(path) => {
            let file = std::fs::File::open(path)?;
            if path.to_ascii_lowercase().ends_with(".zst") {
                read_chunks(
                    &mut zstd::stream::read::Decoder::new(file)?,
                    source.format,
                    &mut callback,
                )?;
            } else {
                read_chunks(
                    &mut std::io::BufReader::new(file),
                    source.format,
                    &mut callback,
                )?;
            }
            Ok(())
        }
        Input::Stdin => {
            read_chunks(&mut std::io::stdin().lock(), source.format, &mut callback)?;
            Ok(())
        }
        Input::Tcp(host, port) => {
            // Desperado 0.4's TCP reader drops an incomplete final chunk.
            // Preserve complete samples at EOF, with the same bounded framing
            // used for files, while allowing interactive/interrupt polling.
            use tokio::io::AsyncReadExt;
            let mut socket = tokio::net::TcpStream::connect((host.as_str(), *port)).await?;
            let mut bytes = vec![0; CHUNK_SAMPLES * sample_bytes(source.format)];
            let mut used = 0;
            loop {
                tokio::select! {
                    n=socket.read(&mut bytes[used..])=> {
                        let n=n?;
                        if n==0 {
                            if used>0 {callback(convert(&bytes[..used],source.format)?)?;}
                            return Ok(());
                        }
                        used+=n;
                        if used==bytes.len() {
                            if !callback(convert(&bytes,source.format)?)? {return Ok(());}
                            used=0;
                        }
                    },
                    _=tokio::time::sleep(std::time::Duration::from_millis(50))=>if !callback(vec![])? {return Ok(());},
                }
            }
        }
        #[cfg(any(
            feature = "hackrf",
            feature = "soapy",
            feature = "airspy",
            feature = "pluto"
        ))]
        Input::Device(config) => {
            let mut stream = desperado::IqAsyncSource::from_device_config(config).await?;
            let run_result: Result<(), SourceError> = async {
                loop {
                    tokio::select! {
                        chunk=stream.next()=>match chunk {Some(chunk)=>if !callback(chunk?)? {break;},None=>break},
                        _=tokio::time::sleep(std::time::Duration::from_millis(50))=>if !callback(vec![])? {break;},
                    }
                }
                Ok(())
            }.await;
            let stop_result = stream.stop().await;
            run_result?;
            stop_result?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rtl_sdr_uri_reports_unsupported_rate() {
        let error = "rtlsdr://0?freq=1030M&rate=2.4M"
            .parse::<Source>()
            .unwrap_err()
            .to_string();
        assert!(error.contains("RTL-SDR is not supported"));
        assert!(error.contains("8 MS/s"));
        assert!(!error.contains("rebuild"));
    }
    #[test]
    fn paths_and_gqrx_metadata() {
        let source: Source =
            "/tmp/Mobile Documents/gqrx_20261006_105707_1033326000_16000000_fc.raw.zst"
                .parse()
                .unwrap();
        assert_eq!(source.sample_rate, 16_000_000);
        assert_eq!(source.center_frequency, 1_033_326_000);
        assert_eq!(source.start_timestamp, Some(1791284227.));
        let uri: Source = "file:///tmp/Mobile%20Documents/test.cf32?rate=8M"
            .parse()
            .unwrap();
        assert!(matches!(uri.input, Input::File(p) if p == "/tmp/Mobile Documents/test.cf32"));
        assert_eq!(uri.sample_rate, 8_000_000);
    }
    #[test]
    fn partial_reads_and_bad_tail() {
        struct ShortReader(std::io::Cursor<Vec<u8>>);
        impl Read for ShortReader {
            fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
                let n = b.len().min(3);
                self.0.read(&mut b[..n])
            }
        }
        let bytes: Vec<_> = [1f32, -2., 3.5, 4.25]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        let mut got = vec![];
        read_chunks(
            &mut ShortReader(std::io::Cursor::new(bytes.clone())),
            IqFormat::Cf32,
            &mut |v| {
                got.extend(v);
                Ok(true)
            },
        )
        .unwrap();
        assert_eq!(
            got,
            vec![Complex32::new(1., -2.), Complex32::new(3.5, 4.25)]
        );
        assert!(read_chunks(
            &mut std::io::Cursor::new(&bytes[..15]),
            IqFormat::Cf32,
            &mut |_| Ok(true)
        )
        .is_err());
    }
    #[test]
    fn compressed_and_other_formats() {
        let bytes: Vec<_> = [1f32, -2., 3.5, 4.25]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        let compressed = zstd::stream::encode_all(bytes.as_slice(), 1).unwrap();
        let mut got = vec![];
        read_chunks(
            &mut zstd::stream::read::Decoder::new(compressed.as_slice()).unwrap(),
            IqFormat::Cf32,
            &mut |v| {
                got.extend(v);
                Ok(true)
            },
        )
        .unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(
            convert(&[0, 128], IqFormat::Cs8).unwrap(),
            vec![Complex32::new(0., -1.)]
        );
        let nonfinite: Vec<_> = [f32::NAN, 0.0]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        assert!(convert(&nonfinite, IqFormat::Cf32).is_err());
    }
    #[cfg(feature = "hackrf")]
    #[test]
    fn device_overrides_reconfigure_hardware() {
        let mut s: Source = "hackrf://?freq=1032M&rate=16M&lna_gain=16&vga_gain=34"
            .parse()
            .unwrap();
        s.configure(Some(8_000_000), Some(1_030_000_000), None);
        match s.input {
            Input::Device(desperado::DeviceConfig::HackRf(c)) => {
                assert_eq!(c.sample_rate, 8_000_000);
                assert_eq!(c.center_freq, 1_030_000_000);
            }
            _ => panic!(),
        }
    }
}
