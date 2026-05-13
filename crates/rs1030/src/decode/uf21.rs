//! UF21 — Comm-A identity request.
//!
//! A 112-bit selectively addressed Mode S uplink interrogation carrying a
//! 56-bit Comm-A `MA` message field.
//!
//! References:
//!
//! - ICAO Annex 10, Vol. IV, Figure 3-7: uplink format summary.
//! - ICAO Annex 10, Vol. IV, §3.1.2.6.4: Comm-A identity request, UF21.
//! - ICAO Annex 10, Vol. IV, §3.1.2.6.11.1: Comm-A protocol.
//!
//! | Bits | Field | Size | Meaning |
//! | ---: | --- | ---: | --- |
//! | 1-5 | `UF` | 5 | Uplink format, fixed to `21` (`10101`) |
//! | 6-8 | `PC` | 3 | Protocol control |
//! | 9-13 | `RR` | 5 | Reply request |
//! | 14-16 | `DI` | 3 | Designator identification |
//! | 17-32 | `SD` | 16 | Special designator |
//! | 33-88 | `MA` | 56 | Comm-A message field |
//! | 89-112 | `AP` | 24 | Address/parity overlay field |

use deku::prelude::*;
use serde::Serialize;

use crate::decode::util::{queried_bds, BdsCode};
use crate::decode::util::Ma;
use crate::decode::util::SpecialDesignator;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, DekuRead)]
pub struct Uf21 {
    #[deku(bits = "5")]
    #[serde(skip)]
    /// Uplink format descriptor. For UF21 this is always `21` (`10101`).
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

    #[deku(bits = "24", endian = "big")]
    #[serde(skip)]
    /// Raw `AP`, address/parity overlay field, Annex bits 89-112.
    pub ap: u32,
}

pub fn decode(frame: &[u8]) -> Result<Uf21, DekuError> {
    let (_, parsed) = Uf21::from_bytes((frame, 0))?;
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_layout() {
        let frame = [
            0xa8, 0x00, 0x00, 0x00, 1, 2, 3, 4, 5, 6, 7, 0x12, 0x34, 0x56,
        ];
        let parsed = decode(&frame).unwrap();
        assert_eq!(parsed.uf, 21);
        assert_eq!(parsed.ap, 0x123456);
        assert!(matches!(parsed.ma, Ma::DifferentialGpsCorrection { .. }));
    }
}
