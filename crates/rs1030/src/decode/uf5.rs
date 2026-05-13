//! UF5 — surveillance identity request.
//!
//! A 56-bit selectively addressed Mode S uplink interrogation requesting an
//! identity surveillance reply or Comm-B identity reply, depending on `RR`.
//!
//! References:
//!
//! - ICAO Annex 10, Vol. IV, Figure 3-7: uplink format summary.
//! - ICAO Annex 10, Vol. IV, §3.1.2.6.3: surveillance identity request, UF5.
//! - ICAO Annex 10, Vol. IV, §3.1.2.6.1.2: `RR` and queried BDS rules.
//!
//! | Bits | Field | Size | Meaning |
//! | ---: | --- | ---: | --- |
//! | 1-5 | `UF` | 5 | Uplink format, fixed to `5` (`00101`) |
//! | 6-8 | `PC` | 3 | Protocol control |
//! | 9-13 | `RR` | 5 | Reply request (`0..=15` → DF5, `16..=31` → DF21) |
//! | 14-16 | `DI` | 3 | Designator identification; defines the `SD` structure |
//! | 17-32 | `SD` | 16 | Special designator |
//! | 33-56 | `AP` | 24 | Address/parity overlay field |

use deku::prelude::*;
use serde::Serialize;

use crate::decode::util::{queried_bds, BdsCode};
use crate::decode::util::SpecialDesignator;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, DekuRead)]
pub struct Uf5 {
    #[deku(bits = "5")]
    #[serde(skip)]
    /// Uplink format descriptor. For UF5 this is always `5` (`00101`).
    pub uf: u8,

    #[deku(bits = "3")]
    /// `PC`, protocol control, Annex bits 6-8.
    pub pc: u8,

    #[deku(bits = "5")]
    /// `RR`, reply request, Annex bits 9-13.
    ///
    /// `0..=15` requests DF5 surveillance reply; `16..=31` requests DF21
    /// Comm-B identity reply and defines `BDS1` through the low nibble.
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

    #[deku(bits = "24", endian = "big")]
    #[serde(skip)]
    /// Raw `AP`, address/parity overlay field, Annex bits 33-56.
    pub ap: u32,
}

pub fn decode(frame: &[u8]) -> Result<Uf5, DekuError> {
    let (_, parsed) = Uf5::from_bytes((frame, 0))?;
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_layout() {
        let frame = [0x28, 0x00, 0x00, 0x00, 0x12, 0x34, 0x56];
        let parsed = decode(&frame).unwrap();
        assert_eq!(parsed.uf, 5);
        assert_eq!(parsed.pc, 0);
        assert_eq!(parsed.rr, 0);
        assert_eq!(parsed.di, 0);
        assert_eq!(parsed.bds, None);
        assert_eq!(parsed.ap, 0x123456);
    }
}
