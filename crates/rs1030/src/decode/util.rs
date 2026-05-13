//! Shared decode utilities for UF4/UF5/UF20/UF21.
//!
//! This module groups the common data-link structures used by surveillance and
//! Comm-A uplink formats:
//!
//! - `SD`, the DI-dispatched special designator field.
//! - `BDS`, the queried Comm-B register derived from `RR` and `SD/RRS`.
//! - `MA`, the 56-bit Comm-A message field for UF20/UF21.
//!
//! References:
//!
//! - ICAO Annex 10, Vol. IV, §3.1.2.6.1.2: `RR` and queried BDS rules.
//! - ICAO Annex 10, Vol. IV, §3.1.2.6.1.3-4: `DI` and `SD`.
//! - ICAO Annex 10, Vol. IV, §3.1.2.6.2.1: `MA`, Comm-A message field.
//! - ICAO Annex 10, Vol. IV, §3.1.2.6.11.1: Comm-A protocol.
//! - ICAO Doc 9871 Appendix A.3.2.2: uplink MSP channel 2, TIS.
//! - ICAO Doc 9871 Appendix A.3.2.5: uplink MSP channel 5, ACAS sensitivity level control.
//! - DO-181E Appendix C, §C.2.2.7.1.1.1: linked Comm-A `IIS`/`LAS` use in `SD`.
//! - DO-181E Appendix C, Table C-2-3: uplink broadcast identifiers.
//! - DO-181E §2.2.22.1.1: TCAS sensitivity level command `ADS`/`SLC` fields.

use std::fmt;

use deku::prelude::*;
use serde::{Deserialize, Deserializer, Serialize};

/// Recovered 24-bit address from an uplink `AP` address/parity field.
///
/// This mirrors `jet1090`'s `IcaoParity`: the raw 24 parity bits are consumed
/// from the frame, but the stored value comes from decoder context. For 1030 MHz
/// uplink, that context is computed by removing the Mode S CRC contribution and
/// inverting the uplink AP overlay transform.
#[derive(PartialEq, Eq, PartialOrd, DekuRead, Hash, Copy, Clone, Ord)]
#[deku(ctx = "address: u32")]
pub struct Icao24(
    /// Six-hex-digit Mode S address recovered from the `AP` overlay.
    #[deku(bits = "24", map = "|_v: u32| -> Result<_, DekuError> { Ok(address) }")]
    pub u32,
);

impl fmt::Debug for Icao24 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:06x}", self.0)
    }
}

impl fmt::Display for Icao24 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:06x}", self.0)
    }
}

impl Serialize for Icao24 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&format!("{:06x}", self.0))
    }
}

impl<'de> Deserialize<'de> for Icao24 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        <Icao24 as core::str::FromStr>::from_str(&s).map_err(serde::de::Error::custom)
    }
}

impl core::str::FromStr for Icao24 {
    type Err = core::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let num = u32::from_str_radix(s, 16)?;
        Ok(Self(num))
    }
}

// -----------------------------------------------------------------------------
// SD: special designator
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, DekuRead)]
#[deku(ctx = "di: u8", id = "di")]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SpecialDesignator {
    /// `DI = 0`: II-code interrogator identifier plus overlay control.
    #[deku(id = "0")]
    Di0(SdDi0),

    /// `DI = 1`: multisite and communications control information.
    #[deku(id = "1")]
    Di1(SdDi1),

    /// `DI = 2`: extended-squitter control information.
    #[deku(id = "2")]
    Di2(SdDi2),

    /// `DI = 3`: SI multisite lockout, broadcast and GICB control information.
    #[deku(id = "3")]
    Di3(SdDi3),

    /// `DI = 7`: extended data readout request and communications control.
    #[deku(id = "7")]
    Di7(SdDi7),

    /// `DI = 4, 5, 6`: reserved by Annex 10; retain the uninterpreted bits.
    #[deku(id_pat = "_")]
    Reserved {
        /// `DI` value that selected this reserved structure.
        di: u8,
        /// Raw 16-bit `SD` field, Annex bits 17-32.
        #[deku(bits = "16", endian = "big")]
        bits: u16,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, DekuRead)]
pub struct SdDi0 {
    /// `IIS`, interrogator identifier subfield, Annex bits 17-20.
    #[deku(bits = "4")]
    pub iis: u8,

    /// Reserved bits 21-27, retained for diagnostics.
    #[deku(bits = "7")]
    pub reserved_21_27: u8,

    /// `OVC`, overlay control, Annex bit 28.
    #[deku(bits = "1")]
    pub ovc: bool,

    /// Reserved bits 29-32, retained for diagnostics.
    #[deku(bits = "4")]
    pub reserved_29_32: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, DekuRead)]
pub struct SdDi1 {
    /// `IIS`, interrogator identifier subfield, Annex bits 17-20.
    #[deku(bits = "4")]
    pub iis: u8,

    /// `MBS`, multisite Comm-B subfield, Annex bits 21-22.
    #[deku(bits = "2")]
    pub mbs: u8,

    /// `MES`, multisite ELM subfield, Annex bits 23-25.
    #[deku(bits = "3")]
    pub mes: u8,

    /// `LOS`, lockout subfield, Annex bit 26.
    #[deku(bits = "1")]
    pub los: bool,

    /// `RSS`, reservation status subfield, Annex bits 27-28.
    #[deku(bits = "2")]
    pub rss: u8,

    /// `TMS`, tactical message subfield, Annex bits 29-32.
    #[deku(bits = "4")]
    pub tms: u8,
}

impl SdDi1 {
    /// `LAS`, linked Comm-A subfield, DO-181E Appendix C bits 30-32.
    pub fn las(&self) -> u8 {
        self.tms & 0x07
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, DekuRead)]
pub struct SdDi2 {
    /// Reserved bits 17-20.
    #[deku(bits = "4")]
    pub reserved_17_20: u8,

    /// `TCS`, type control subfield, Annex bits 21-23.
    #[deku(bits = "3")]
    pub tcs: u8,

    /// `RCS`, rate control subfield, Annex bits 24-26.
    #[deku(bits = "3")]
    pub rcs: u8,

    /// `SAS`, surface antenna subfield, Annex bits 27-28.
    #[deku(bits = "2")]
    pub sas: u8,

    /// Reserved bits 29-32.
    #[deku(bits = "4")]
    pub reserved_29_32: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, DekuRead)]
pub struct SdDi3 {
    /// `SIS`, surveillance identifier subfield, Annex bits 17-22.
    #[deku(bits = "6")]
    pub sis: u8,

    /// `LSS`, lockout surveillance subfield, Annex bit 23.
    #[deku(bits = "1")]
    pub lss: bool,

    /// `RRS`, reply request subfield carrying BDS2, Annex bits 24-27.
    #[deku(bits = "4")]
    pub rrs: u8,

    /// `OVC`, overlay control, Annex bit 28.
    #[deku(bits = "1")]
    pub ovc: bool,

    /// Reserved bits 29-32.
    #[deku(bits = "4")]
    pub reserved_29_32: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, DekuRead)]
pub struct SdDi7 {
    /// `IIS`, interrogator identifier subfield, Annex bits 17-20.
    #[deku(bits = "4")]
    pub iis: u8,

    /// `RRS`, reply request subfield carrying BDS2, Annex bits 21-24.
    #[deku(bits = "4")]
    pub rrs: u8,

    /// Reserved bit 25.
    #[deku(bits = "1")]
    pub reserved_25: bool,

    /// `LOS`, lockout subfield, Annex bit 26.
    #[deku(bits = "1")]
    pub los: bool,

    /// Reserved bit 27.
    #[deku(bits = "1")]
    pub reserved_27: bool,

    /// `OVC`, overlay control, Annex bit 28.
    #[deku(bits = "1")]
    pub ovc: bool,

    /// `TMS`, tactical message subfield, Annex bits 29-32.
    #[deku(bits = "4")]
    pub tms: u8,
}

impl SdDi7 {
    /// `LAS`, linked Comm-A subfield, DO-181E Appendix C bits 30-32.
    pub fn las(&self) -> u8 {
        self.tms & 0x07
    }
}

pub fn decode_sd(di: u8, bits: u16) -> Result<SpecialDesignator, DekuError> {
    let bytes = bits.to_be_bytes();
    let mut cursor = deku::no_std_io::Cursor::new(bytes);
    let reader = &mut deku::reader::Reader::new(&mut cursor);
    SpecialDesignator::from_reader_with_ctx(reader, di)
}

#[cfg(test)]
mod sd_tests {
    use super::*;

    #[test]
    fn decodes_di7_rrs_and_las() {
        let sd = decode_sd(7, 0x2a55).unwrap();
        match sd {
            SpecialDesignator::Di7(value) => {
                assert_eq!(value.iis, 2);
                assert_eq!(value.rrs, 0x0a);
                assert!(value.los);
                assert!(value.ovc);
                assert_eq!(value.las(), 5);
            }
            _ => panic!("wrong SD variant"),
        }
    }
}

// -----------------------------------------------------------------------------
// BDS: queried Comm-B register
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BdsCode(pub u8);

impl Serialize for BdsCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&format!("0x{:02x}", self.0))
    }
}

pub fn queried_bds(rr: u8, sd: &SpecialDesignator) -> Option<BdsCode> {
    if rr < 16 {
        return None;
    }

    let bds1 = rr & 0x0f;
    let bds2 = match sd {
        SpecialDesignator::Di3(value) => value.rrs,
        SpecialDesignator::Di7(value) => value.rrs,
        _ => 0,
    };

    Some(BdsCode((bds1 << 4) | bds2))
}

#[cfg(test)]
mod bds_tests {
    use super::*;
    use crate::decode::util::decode_sd;

    #[test]
    fn no_bds_for_surveillance_request() {
        let sd = decode_sd(0, 0).unwrap();
        assert_eq!(queried_bds(0, &sd), None);
    }

    #[test]
    fn bds2_defaults_to_zero_without_rrs() {
        let sd = decode_sd(0, 0).unwrap();
        assert_eq!(queried_bds(18, &sd), Some(BdsCode(0x20)));
    }

    #[test]
    fn bds2_comes_from_di3_rrs() {
        // DI=3: RRS is Annex bits 24-27, i.e. bits 8..11 of this u16 value.
        let sd = decode_sd(3, 0x00a0).unwrap();
        assert_eq!(queried_bds(18, &sd), Some(BdsCode(0x25)));
    }
}

// -----------------------------------------------------------------------------
// MA: Comm-A message
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, DekuRead)]
#[deku(id_type = "u8")]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Ma {
    /// TIS uplink message, MSP channel 2 / header `0x02`.
    #[deku(id = "0x02")]
    TisUplink(TisUplink),

    /// TCAS/ACAS sensitivity level command, ADS/header `0x05`.
    #[deku(id = "0x05")]
    TcasSensitivityLevelCommand(TcasSensitivityLevelCommand),

    /// Differential GPS correction broadcast, uplink broadcast identifier `0x01`.
    #[deku(id = "0x01")]
    DifferentialGpsCorrection { payload: [u8; 6] },

    /// TCAS/ACAS RA broadcast placeholder, uplink broadcast identifier `0x31`.
    #[deku(id = "0x31")]
    TcasRaBroadcast { payload: [u8; 6] },

    /// TCAS/ACAS broadcast placeholder, uplink broadcast identifier `0x32`.
    #[deku(id = "0x32")]
    AcasBroadcast { payload: [u8; 6] },

    /// Unknown or not-yet-decoded Comm-A message. `id` is the first MA byte.
    #[deku(id_pat = "_")]
    Unknown {
        #[serde(serialize_with = "serialize_hex_u8")]
        id: u8,
        payload: [u8; 6],
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, DekuRead)]
pub struct TisUplink {
    /// TIS message type, Doc 9871 §A.3.2.2.2.2, bits 9-14 of `MA`.
    #[deku(bits = "6")]
    pub message_type: u8,

    /// Interpreted TIS message kind derived from `message_type`.
    #[deku(skip, default = "TisMessageKind::from_message_type(*message_type)")]
    pub kind: TisMessageKind,

    /// First 21-bit TIS traffic information block, Doc 9871 §A.3.2.2.2.3.
    pub block1: TisTrafficBlock,

    /// Second 21-bit TIS traffic information block, Doc 9871 §A.3.2.2.2.3.
    pub block2: TisTrafficBlock,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TisMessageKind {
    /// Traffic data first segment; value carries own heading in 6 degree steps.
    TrafficDataFirstSegment { own_heading_degrees: u16 },
    /// Traffic data intermediate segment, message type 60.
    TrafficDataIntermediateSegment,
    /// Traffic data final segment, message type 61.
    TrafficDataFinalSegment,
    /// TIS goodbye message, message type 62.
    Goodbye,
    /// TIS keep-alive message, message type 63.
    KeepAlive,
}

impl TisMessageKind {
    fn from_message_type(message_type: u8) -> Self {
        match message_type {
            0..=59 => Self::TrafficDataFirstSegment {
                own_heading_degrees: message_type as u16 * 6,
            },
            60 => Self::TrafficDataIntermediateSegment,
            61 => Self::TrafficDataFinalSegment,
            62 => Self::Goodbye,
            63 => Self::KeepAlive,
            _ => unreachable!("6-bit message type cannot exceed 63"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, DekuRead)]
pub struct TisTrafficBlock {
    /// Traffic bearing from own-aircraft heading, 6-degree units. Value 63 means null alert.
    #[deku(bits = "6")]
    pub bearing: u8,

    /// Traffic range code. Not present/meaningful when `bearing == 63`.
    #[deku(bits = "4")]
    pub range: u8,

    /// Relative altitude code. Not present/meaningful when `bearing == 63`.
    #[deku(bits = "5")]
    pub relative_altitude: u8,

    /// Altitude-rate code. Not present/meaningful when `bearing == 63`.
    #[deku(bits = "2")]
    pub altitude_rate: u8,

    /// Traffic heading code, 45-degree units. Not present/meaningful when `bearing == 63`.
    #[deku(bits = "3")]
    pub heading: u8,

    /// Traffic status: false = proximity alert, true = threat alert.
    #[deku(bits = "1")]
    pub threat: bool,
}

impl TisTrafficBlock {
    pub fn is_null_alert(&self) -> bool {
        self.bearing == 63
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, DekuRead)]
pub struct TcasSensitivityLevelCommand {
    /// `SLC`, TCAS sensitivity level command, DO-181E §2.2.22.1.1 bits 41-44.
    #[deku(bits = "4")]
    pub slc: u8,

    /// Remaining low nibble of the second MA byte, retained until fully specified.
    #[deku(bits = "4")]
    pub reserved_45_48: u8,

    /// Remaining bytes of the 56-bit `MA` field after `ADS=0x05` and `SLC`.
    pub payload: [u8; 5],
}

fn serialize_hex_u8<S>(value: &u8, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&format!("0x{value:02x}"))
}

#[cfg(test)]
mod ma_tests {
    use super::*;

    #[test]
    fn decodes_tcas_slc() {
        let (_, ma) = Ma::from_bytes((&[0x05, 0xa0, 0, 0, 0, 0, 0], 0)).unwrap();
        assert!(matches!(
            ma,
            Ma::TcasSensitivityLevelCommand(TcasSensitivityLevelCommand { slc: 10, .. })
        ));
    }

    #[test]
    fn decodes_tis_keep_alive() {
        let (_, ma) = Ma::from_bytes((&[0x02, 0xfc, 0, 0, 0, 0, 0], 0)).unwrap();
        match ma {
            Ma::TisUplink(tis) => {
                assert_eq!(tis.message_type, 63);
                assert_eq!(tis.kind, TisMessageKind::KeepAlive);
            }
            _ => panic!("wrong MA variant"),
        }
    }
}
