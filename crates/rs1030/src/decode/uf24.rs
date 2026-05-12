//! UF24 — Comm-C / extended length message (ELM).
//!
//! A 112-bit Mode S uplink interrogation carrying an 80-bit Comm-C `MC`
//! message field. Unlike most uplink formats, UF24 is identified by the first
//! two bits `11`; the following bits are content (`RC`/`NC`) rather than a
//! conventional five-bit UF number.
//!
//! References:
//!
//! - ICAO Annex 10, Vol. IV, Figure 3-7: uplink format summary.
//! - ICAO Annex 10, Vol. IV, §3.1.2.7.1: Comm-C, UF24.
//!
//! | Bits | Field | Size | Meaning |
//! | ---: | --- | ---: | --- |
//! | 1-2 | `UF` prefix | 2 | Fixed to `11` |
//! | 3-4 | `RC` | 2 | Reply control |
//! | 5-8 | `NC` | 4 | Number of C-segment |
//! | 9-88 | `MC` | 80 | Comm-C message field |
//! | 89-112 | `AP` | 24 | Address/parity overlay field |

use deku::prelude::*;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, DekuRead)]
pub struct Uf24 {
    #[deku(bits = "2")]
    #[serde(skip)]
    /// UF24 prefix. Always `3` binary `11`.
    pub uf_prefix: u8,

    #[deku(bits = "2")]
    /// Reply control, Annex bits 3-4.
    pub rc: u8,

    #[deku(bits = "4")]
    /// Number of C-segment, Annex bits 5-8.
    pub nc: u8,

    /// Comm-C message field, Annex bits 9-88.
    pub mc: [u8; 10],

    #[deku(bits = "24", endian = "big")]
    #[serde(skip)]
    /// Raw address/parity field, Annex bits 89-112.
    pub ap: u32,
}

pub fn decode(frame: &[u8]) -> Result<Uf24, DekuError> {
    let (_, parsed) = Uf24::from_bytes((frame, 0))?;
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_layout() {
        let frame = [0xc0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 0x12, 0x34, 0x56];
        let parsed = decode(&frame).unwrap();
        assert_eq!(parsed.uf_prefix, 3);
        assert_eq!(parsed.rc, 0);
        assert_eq!(parsed.nc, 0);
        assert_eq!(parsed.mc, [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        assert_eq!(parsed.ap, 0x123456);
    }
}
