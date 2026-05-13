//! UF4 — surveillance altitude request.
//!
//! A 56-bit selectively addressed Mode S uplink interrogation requesting an
//! altitude surveillance reply or Comm-B altitude reply, depending on `RR`.
//!
//! References:
//!
//! - ICAO Annex 10, Vol. IV, Figure 3-7: uplink format summary.
//! - ICAO Annex 10, Vol. IV, §3.1.2.6.1: surveillance altitude request, UF4.
//! - ICAO Annex 10, Vol. IV, §3.1.2.6.1.2: `RR` and queried BDS rules.
//!
//! | Bits | Field | Size | Meaning |
//! | ---: | --- | ---: | --- |
//! | 1-5 | `UF` | 5 | Uplink format, fixed to `4` (`00100`) |
//! | 6-8 | `PC` | 3 | Protocol control |
//! | 9-13 | `RR` | 5 | Reply request (`0..=15` → DF4, `16..=31` → DF20) |
//! | 14-16 | `DI` | 3 | Designator identification; defines the `SD` structure |
//! | 17-32 | `SD` | 16 | Special designator |
//! | 33-56 | `AP` | 24 | Address/parity overlay field |

use deku::prelude::*;
use serde::Serialize;

use crate::decode::util::{queried_bds, BdsCode};
use crate::decode::util::SpecialDesignator;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, DekuRead)]
pub struct Uf4 {
    #[deku(bits = "5")]
    #[serde(skip)]
    /// Uplink format descriptor. For UF4 this is always `4` (`00100`).
    pub uf: u8,

    #[deku(bits = "3")]
    /// `PC`, protocol control, Annex bits 6-8.
    pub pc: u8,

    #[deku(bits = "5")]
    /// `RR`, reply request, Annex bits 9-13.
    ///
    /// `0..=15` requests DF4 surveillance reply; `16..=31` requests DF20
    /// Comm-B altitude reply and defines `BDS1` through the low nibble.
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

pub fn decode(frame: &[u8]) -> Result<Uf4, DekuError> {
    let (_, parsed) = Uf4::from_bytes((frame, 0))?;
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_layout() {
        let frame = [0x20, 0x00, 0x00, 0x00, 0x12, 0x34, 0x56];
        let parsed = decode(&frame).unwrap();
        assert_eq!(parsed.uf, 4);
        assert_eq!(parsed.pc, 0);
        assert_eq!(parsed.rr, 0);
        assert_eq!(parsed.di, 0);
        assert_eq!(parsed.bds, None);
        assert_eq!(parsed.ap, 0x123456);
    }

    #[test]
    fn computes_bds() {
        let frame = [0x20, 0x90, 0x00, 0x00, 0x12, 0x34, 0x56];
        let parsed = decode(&frame).unwrap();
        assert_eq!(parsed.bds, Some(BdsCode(0x20)));
    }
}
