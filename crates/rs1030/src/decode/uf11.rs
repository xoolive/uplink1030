//! UF11 — Mode S-only all-call interrogation.
//!
//! A 56-bit Mode S uplink all-call interrogation. The AP field carries the
//! all-call address after uplink AP overlay processing.
//!
//! References:
//!
//! - ICAO Annex 10, Vol. IV, Figure 3-7: uplink format summary.
//! - ICAO Annex 10, Vol. IV, §3.1.2.5.2.1: Mode S-only all-call fields.
//! - ICAO Annex 10, Vol. IV, §3.1.2.4.1.2.3.1.2: all-call address.
//!
//! | Bits | Field | Size | Meaning |
//! | ---: | --- | ---: | --- |
//! | 1-5 | `UF` | 5 | Uplink format, fixed to `11` (`01011`) |
//! | 6-9 | `PR` | 4 | Reply probability |
//! | 10-13 | `IC` | 4 | Interrogator code |
//! | 14-16 | `CL` | 3 | Code label; defines how `IC` is interpreted |
//! | 17-32 | spare | 16 | Unassigned, transmitted as zero |
//! | 33-56 | `AP` | 24 | Address/parity overlay field |

use deku::prelude::*;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, DekuRead)]
pub struct Uf11 {
    #[deku(bits = "5")]
    #[serde(skip)]
    /// Uplink format descriptor. For UF11 this is always `11` (`01011`).
    pub uf: u8,

    #[deku(bits = "4")]
    /// Reply probability, Annex bits 6-9.
    pub pr: u8,

    #[deku(bits = "4")]
    /// Interrogator code, Annex bits 10-13.
    pub ic: u8,

    #[deku(bits = "3")]
    /// Code label, Annex bits 14-16. Defines how `IC` is interpreted.
    pub cl: u8,

    #[deku(bits = "16", endian = "big")]
    #[serde(skip)]
    /// Spare field, Annex bits 17-32.
    pub spare: u16,

    #[deku(bits = "24", endian = "big")]
    #[serde(skip)]
    /// Raw address/parity field, Annex bits 33-56.
    pub ap: u32,
}

pub fn decode(frame: &[u8]) -> Result<Uf11, DekuError> {
    let (_, parsed) = Uf11::from_bytes((frame, 0))?;
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_layout() {
        let frame = [0x58, 0x00, 0x00, 0x00, 0x12, 0x34, 0x56];
        let parsed = decode(&frame).unwrap();
        assert_eq!(parsed.uf, 11);
        assert_eq!(parsed.pr, 0);
        assert_eq!(parsed.ic, 0);
        assert_eq!(parsed.cl, 0);
        assert_eq!(parsed.ap, 0x123456);
    }
}
