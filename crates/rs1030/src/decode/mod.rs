//! High-level decoded Mode S 1030 MHz uplink frames.
//!
//! Format-specific decoders are organized by uplink format (`uf0.rs`,
//! `uf16.rs`, etc.). This module dispatches based on the `UF` field and adds
//! common metadata such as raw frame hex. Recovered AP-overlay addresses are
//! decoded inside the UF payloads, mirroring `jet1090`'s DF-specific
//! `IcaoParity` handling.
//!
//! Primary references:
//!
//! - ICAO Annex 10, Vol. IV, §3.1.2.3: Mode S data encoding and AP parity.
//! - ICAO Annex 10, Vol. IV, Figure 3-7: uplink format summary.

pub mod uf0;
pub mod uf11;
pub mod uf16;
pub mod uf20;
pub mod uf21;
pub mod uf24;
pub mod uf4;
pub mod uf5;
pub mod util;

use deku::{ctx::Order, prelude::*};
use serde::Serialize;

use crate::bits::bytes_to_hex;
use crate::crc::recover_ap_address;
use crate::decode::uf0::Uf0;
use crate::decode::uf11::Uf11;
use crate::decode::uf16::Uf16;
use crate::decode::uf20::Uf20;
use crate::decode::uf21::Uf21;
use crate::decode::uf24::Uf24;
use crate::decode::uf4::Uf4;
use crate::decode::uf5::Uf5;

/// Mode S uplink format (`UF`) and decoded payload.
///
/// This enum is both the UF discriminator and the decoded format-specific
/// payload. It is manually `DekuRead` because UF24 is identified by the first
/// two bits `11`, while other formats use a conventional five-bit UF value.
///
/// Reference: ICAO Annex 10, Vol. IV, §3.1.2.3.2.1.1 and Figure 3-7.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UplinkFormat {
    /// UF0: short air-air surveillance / ACAS, 56 bits.
    Uf0(Uf0),
    /// UF4: surveillance altitude request, 56 bits.
    Uf4(Uf4),
    /// UF5: surveillance identity request, 56 bits.
    Uf5(Uf5),
    /// UF11: Mode S-only all-call, 56 bits.
    Uf11(Uf11),
    /// UF16: long air-air surveillance / ACAS, 112 bits.
    Uf16(Uf16),
    /// UF20: Comm-A altitude request, 112 bits.
    Uf20(Uf20),
    /// UF21: Comm-A identity request, 112 bits.
    Uf21(Uf21),
    /// UF24: Comm-C / extended length message, 112 bits.
    Uf24(Uf24),
    /// Reserved or currently unsupported uplink format.
    Unknown { uf: u8 },
}

impl UplinkFormat {
    pub fn number(&self) -> u8 {
        match self {
            Self::Uf0(_) => 0,
            Self::Uf4(_) => 4,
            Self::Uf5(_) => 5,
            Self::Uf11(_) => 11,
            Self::Uf16(_) => 16,
            Self::Uf20(_) => 20,
            Self::Uf21(_) => 21,
            Self::Uf24(_) => 24,
            Self::Unknown { uf } => *uf,
        }
    }

    fn from_frame(frame: &[u8], icao24: u32) -> Result<Self, DekuError> {
        let first_five = frame[0] >> 3;
        match first_five {
            0 => Ok(Self::Uf0(uf0::decode(frame, icao24)?)),
            4 => Ok(Self::Uf4(uf4::decode(frame, icao24)?)),
            5 => Ok(Self::Uf5(uf5::decode(frame, icao24)?)),
            11 => Ok(Self::Uf11(uf11::decode(frame, icao24)?)),
            16 => Ok(Self::Uf16(uf16::decode(frame, icao24)?)),
            20 => Ok(Self::Uf20(uf20::decode(frame, icao24)?)),
            21 => Ok(Self::Uf21(uf21::decode(frame, icao24)?)),
            _ if (frame[0] & 0xC0) == 0xC0 => Ok(Self::Uf24(uf24::decode(frame, icao24)?)),
            other => Ok(Self::Unknown { uf: other }),
        }
    }
}

// The top-level UF dispatcher is implemented manually instead of using
// `#[derive(DekuRead)]`.
//
// A derived Deku enum works best when every variant uses the same discriminator
// width. Mode S uplink does not quite fit that model:
//
// - UF0/4/5/11/16/20/21 use a conventional five-bit UF field.
// - UF24 is special: it is identified by only the first two bits `11`; the next
//   bits are payload fields (`RC` and `NC`), not part of a five-bit UF value.
// - Frame length also depends on the format: UF0/4/5/11 are 56 bits, while
//   UF16/20/21/24 are 112 bits.
//
// Therefore we manually read the first byte, decide short vs long, rebuild the
// full frame, and then delegate to the per-format structs (`Uf0`, `Uf16`, etc.),
// which do use `#[derive(DekuRead)]`.
impl DekuContainerRead<'_> for UplinkFormat {
    fn from_reader<R: deku::no_std_io::Read + deku::no_std_io::Seek>(
        input: (&mut R, usize),
    ) -> Result<(usize, Self), DekuError>
    where
        Self: Sized,
    {
        let reader = &mut deku::reader::Reader::new(input.0);
        if input.1 != 0 {
            reader.skip_bits(input.1, Order::Msb0)?;
        }
        let value = Self::from_reader_with_ctx(reader, ())?;
        Ok((reader.bits_read, value))
    }

    fn from_bytes(input: (&[u8], usize)) -> Result<((&[u8], usize), Self), DekuError>
    where
        Self: Sized,
    {
        let mut cursor = deku::no_std_io::Cursor::new(input.0);
        let reader = &mut deku::reader::Reader::new(&mut cursor);
        if input.1 != 0 {
            reader.skip_bits(input.1, Order::Msb0)?;
        }
        let value = Self::from_reader_with_ctx(reader, ())?;
        let idx = reader.bits_read.div_ceil(8);
        Ok(((&input.0[idx..], reader.bits_read % 8), value))
    }
}

impl DekuReader<'_> for UplinkFormat {
    fn from_reader_with_ctx<R: deku::no_std_io::Read + deku::no_std_io::Seek>(
        reader: &mut deku::reader::Reader<R>,
        _: (),
    ) -> Result<Self, DekuError>
    where
        Self: Sized,
    {
        let first = reader
            .read_bits(8, Order::Msb0)?
            .ok_or_else(|| DekuError::Parse("missing first byte".into()))?
            .into_vec()[0];
        let first_five = first >> 3;
        let bit_len =
            if first_five == 16 || first_five == 20 || first_five == 21 || (first & 0xC0) == 0xC0 {
                112
            } else {
                56
            };
        let mut frame = vec![first];
        if bit_len > 8 {
            let rest = reader
                .read_bits(bit_len - 8, Order::Msb0)?
                .ok_or_else(|| DekuError::Parse("missing frame bytes".into()))?
                .into_vec();
            frame.extend_from_slice(&rest);
        }
        let icao24 = recover_ap_address(&frame, bit_len);
        Self::from_frame(&frame, icao24)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DecodedUplink {
    /// Raw demodulated frame in hexadecimal.
    pub raw: String,

    /// Decoded uplink payload, flattened in JSON.
    #[serde(flatten)]
    pub payload: UplinkFormat,
}

impl DecodedUplink {
    pub fn uf(&self) -> u8 {
        self.payload.number()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    InvalidLength(usize),
    Deku(DekuError),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidLength(len) => write!(f, "invalid Mode S uplink byte length: {len}"),
            Self::Deku(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for DecodeError {}

impl From<DekuError> for DecodeError {
    fn from(value: DekuError) -> Self {
        Self::Deku(value)
    }
}

pub fn decode_frame(frame: &[u8]) -> Result<DecodedUplink, DecodeError> {
    let bits = match frame.len() {
        7 => 56,
        14 => 112,
        len => return Err(DecodeError::InvalidLength(len)),
    };
    let icao24 = recover_ap_address(frame, bits);
    let payload = UplinkFormat::from_frame(frame, icao24)?;

    Ok(DecodedUplink {
        raw: bytes_to_hex(frame),
        payload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_known_uf0() {
        let frame = [0x00, 0x00, 0x00, 0x00, 0x72, 0x19, 0x51];
        let decoded = decode_frame(&frame).unwrap();
        assert_eq!(decoded.uf(), 0);
        match decoded.payload {
            UplinkFormat::Uf0(uf0) => assert_eq!(uf0.ap.0, 0x4b1618),
            _ => panic!("wrong UF"),
        }
    }

    #[test]
    fn dispatches_additional_uplink_formats() {
        let cases: &[(&[u8], u8)] = &[
            (&[0x20, 0, 0, 0, 0x12, 0x34, 0x56], 4),
            (&[0x28, 0, 0, 0, 0x12, 0x34, 0x56], 5),
            (&[0x58, 0, 0, 0, 0x12, 0x34, 0x56], 11),
            (&[0xa0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 0x12, 0x34, 0x56], 20),
            (&[0xa8, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 0x12, 0x34, 0x56], 21),
            (&[0xc0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 0x12, 0x34, 0x56], 24),
        ];
        for (frame, expected_uf) in cases {
            let decoded = decode_frame(frame).unwrap();
            assert_eq!(decoded.uf(), *expected_uf);
            assert!(!matches!(decoded.payload, UplinkFormat::Unknown { .. }));
        }
    }

    #[test]
    fn uplink_format_is_deku_read() {
        let frame = [0x00, 0x00, 0x00, 0x00, 0x72, 0x19, 0x51];
        let (_, payload) = UplinkFormat::from_bytes((&frame, 0)).unwrap();
        assert_eq!(payload.number(), 0);
    }
}
