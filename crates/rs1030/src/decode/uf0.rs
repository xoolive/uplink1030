//! UF0 — short air-air surveillance interrogation (ACAS).
//!
//! A 56-bit Mode S uplink interrogation used by ACAS/TCAS for short air-air
//! surveillance. It is transmitted on 1030 MHz and normally elicits a DF0 or
//! DF16 reply depending on the `RL` bit.
//!
//! References:
//!
//! - ICAO Annex 10, Vol. IV, §3.1.2.3: Mode S data encoding and AP parity.
//! - ICAO Annex 10, Vol. IV, Figure 3-7: uplink format summary.
//! - ICAO Annex 10, Vol. IV, §3.1.2.8.1: UF0 short air-air surveillance.
//! - ICAO Annex 10, Vol. IV, Figure 4-1: ACAS air-air formats.
//!
//! Message structure, Annex bit numbering:
//!
//! | Bits | Field | Size | Meaning |
//! | ---: | --- | ---: | --- |
//! | 1-5 | `UF` | 5 | Uplink format, fixed to `0` (`00000`) |
//! | 6-8 | spare | 3 | Unassigned, transmitted as zero |
//! | 9 | `RL` | 1 | Reply length command: `0` → DF0, `1` → DF16 |
//! | 10-13 | spare | 4 | Unassigned, transmitted as zero |
//! | 14 | `AQ` | 1 | Acquisition bit; controls content of reply `RI` field |
//! | 15-22 | `DS` | 8 | BDS code of requested GICB register for DF16 replies |
//! | 23-32 | spare | 10 | Unassigned, transmitted as zero |
//! | 33-56 | `AP` | 24 | Address/parity overlay field |
//!
//! Notes:
//!
//! - `RL = 0` commands a short DF0 reply.
//! - `RL = 1` commands a long DF16 reply, if supported by the transponder.
//! - `DS` is meaningful when a DF16 reply is requested; it selects the GICB
//!   register whose contents are returned in the corresponding reply.
//! - The raw `AP` field does not directly contain the ICAO address. The address
//!   is recovered by removing the Mode S CRC/parity overlay as defined in
//!   Annex 10, Vol. IV, §3.1.2.3.3.2.

use deku::prelude::*;
use serde::Serialize;

/// UF0 — short air-air surveillance interrogation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, DekuRead)]
pub struct Uf0 {
    #[deku(bits = "5")]
    #[serde(skip)]
    /// Uplink format descriptor. For UF0 this is always `0` (`00000`).
    pub uf: u8,

    #[deku(bits = "3")]
    #[serde(skip)]
    /// Spare bits 6-8. Transmitted as zero.
    pub spare0: u8,

    #[deku(bits = "1")]
    /// Reply length command, Annex bit 9.
    ///
    /// - `0`: request DF0 short air-air surveillance reply
    /// - `1`: request DF16 long air-air surveillance reply
    pub rl: bool,

    #[deku(bits = "4")]
    #[serde(skip)]
    /// Spare bits 10-13. Transmitted as zero.
    pub spare1: u8,

    #[deku(bits = "1")]
    /// Acquisition bit, Annex bit 14.
    ///
    /// Controls the content of the reply `RI` field.
    pub aq: bool,

    /// Data selector, Annex bits 15-22.
    ///
    /// Contains the BDS code of the GICB register requested in a DF16 reply.
    pub ds: u8,

    #[deku(bits = "10")]
    #[serde(skip)]
    /// Spare bits 23-32. Transmitted as zero.
    pub spare2: u16,

    #[deku(bits = "24", endian = "big")]
    #[serde(skip)]
    /// Raw address/parity field, Annex bits 33-56.
    pub ap: u32,
}

pub fn decode(frame: &[u8]) -> Result<Uf0, DekuError> {
    let (_, parsed) = Uf0::from_bytes((frame, 0))?;
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_layout() {
        let frame = [0x00, 0x00, 0x00, 0x00, 0x72, 0x19, 0x51];
        let parsed = decode(&frame).unwrap();
        assert_eq!(parsed.uf, 0);
        assert!(!parsed.rl);
        assert!(!parsed.aq);
        assert_eq!(parsed.ds, 0);
        assert_eq!(parsed.ap, 0x721951);
    }
}
