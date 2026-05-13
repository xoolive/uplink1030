//! UF20 — Comm-A altitude request.
//!
//! A 112-bit selectively addressed Mode S uplink interrogation carrying a
//! 56-bit Comm-A `MA` message field.
//!
//! References:
//!
//! - ICAO Annex 10, Vol. IV, Figure 3-7: uplink format summary.
//! - ICAO Annex 10, Vol. IV, §3.1.2.6.2: Comm-A altitude request, UF20.
//! - ICAO Annex 10, Vol. IV, §3.1.2.6.11.1: Comm-A protocol.
//!
//! | Bits | Field | Size | Meaning |
//! | ---: | --- | ---: | --- |
//! | 1-5 | `UF` | 5 | Uplink format, fixed to `20` (`10100`) |
//! | 6-8 | `PC` | 3 | Protocol control |
//! | 9-13 | `RR` | 5 | Reply request |
//! | 14-16 | `DI` | 3 | Designator identification |
//! | 17-32 | `SD` | 16 | Special designator |
//! | 33-88 | `MA` | 56 | Comm-A message field |
//! | 89-112 | `AP` | 24 | Address/parity overlay field |

use deku::prelude::*;
use serde::Serialize;

use crate::decode::util::Ma;
use crate::decode::util::SpecialDesignator;
use crate::decode::util::{queried_bds, BdsCode, Icao24};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, DekuRead)]
#[deku(ctx = "icao24: u32")]
pub struct Uf20 {
    #[deku(bits = "5")]
    #[serde(skip)]
    /// Uplink format descriptor. For UF20 this is always `20` (`10100`).
    pub uf: u8,

    #[deku(bits = "3")]
    /// `PC`, protocol control, Annex bits 6-8.
    pub pc: u8,

    #[deku(bits = "5")]
    /// `RR`, reply request, Annex bits 9-13.
    pub rr: u8,

    #[deku(bits = "3")]
    /// `DI`, designator identification, Annex bits 14-16. Dispatches `SD`.
    pub di: u8,

    #[deku(ctx = "*di")]
    /// `SD`, special designator, Annex bits 17-32, decoded according to `DI`.
    pub sd: SpecialDesignator,

    #[deku(skip, default = "queried_bds(*rr, &sd)")]
    /// Queried Comm-B register when `RR >= 16`; serialized as hexadecimal.
    pub bds: Option<BdsCode>,

    /// `MA`, 56-bit Comm-A message field, Annex bits 33-88.
    pub ma: Ma,

    #[serde(rename = "icao24")]
    #[deku(ctx = "icao24")]
    /// Recovered address from the `AP` address/parity overlay, Annex bits 89-112.
    pub ap: Icao24,
}

pub fn decode(frame: &[u8], icao24: u32) -> Result<Uf20, DekuError> {
    let mut cursor = deku::no_std_io::Cursor::new(frame);
    let reader = &mut deku::reader::Reader::new(&mut cursor);
    Uf20::from_reader_with_ctx(reader, icao24)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_layout() {
        let frame = [
            0xa0, 0x00, 0x00, 0x00, 1, 2, 3, 4, 5, 6, 7, 0x12, 0x34, 0x56,
        ];
        let parsed = decode(&frame, 0xabcdef).unwrap();
        assert_eq!(parsed.uf, 20);
        assert_eq!(parsed.ap.0, 0xabcdef);
        assert!(matches!(parsed.ma, Ma::DifferentialGpsCorrection { .. }));
    }

    #[test]
    fn computes_bds() {
        let frame = [
            0xa0, 0x90, 0x00, 0x00, 0x05, 0xa0, 0, 0, 0, 0, 0, 0x12, 0x34, 0x56,
        ];
        let parsed = decode(&frame, 0xabcdef).unwrap();
        assert_eq!(parsed.bds, Some(BdsCode(0x20)));
    }
}
