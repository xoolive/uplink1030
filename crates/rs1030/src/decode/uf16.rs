//! UF16 — long air-air surveillance interrogation (ACAS).
//!
//! A 112-bit Mode S uplink interrogation used by ACAS/TCAS for long air-air
//! surveillance and ACAS coordination. It extends UF0 with a 56-bit `MU` field.
//!
//! References:
//!
//! - ICAO Annex 10, Vol. IV, §3.1.2.3: Mode S data encoding and AP parity.
//! - ICAO Annex 10, Vol. IV, Figure 3-7: uplink format summary.
//! - ICAO Annex 10, Vol. IV, Figure 4-1: ACAS air-air formats.
//! - ICAO Annex 10, Vol. IV, §4.3.8.4.2.3: UF16 `MU` field.
//!
//! Message structure, Annex bit numbering:
//!
//! | Bits | Field | Size | Meaning |
//! | ---: | --- | ---: | --- |
//! | 1-5 | `UF` | 5 | Uplink format, fixed to `16` (`10000`) |
//! | 6-8 | spare | 3 | Unassigned, transmitted as zero |
//! | 9 | `RL` | 1 | Reply length command |
//! | 10-13 | spare | 4 | Unassigned, transmitted as zero |
//! | 14 | `AQ` | 1 | Acquisition bit |
//! | 15-32 | spare | 18 | Unassigned in the top-level layout |
//! | 33-88 | `MU` | 56 | ACAS message field |
//! | 89-112 | `AP` | 24 | Address/parity overlay field |
//!
//! `MU` field, Annex 10, Vol. IV, §4.3.8.4.2.3:
//!
//! - Bits 33-40: `UDS`, expressed as `UDS1` and `UDS2` nibbles.
//! - `UDS1 = 3`, `UDS2 = 0`: resolution message.
//! - `UDS1 = 3`, `UDS2 = 1`: RA broadcast.
//! - `UDS1 = 3`, `UDS2 = 2`: ACAS broadcast.

use deku::prelude::*;
use serde::Serialize;

use crate::bits::extract_bits;

/// High-level classification of the UF16 ACAS `MU` field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MuKind {
    /// `UDS1 = 3`, `UDS2 = 0`.
    ///
    /// Carries an ACAS resolution message. Annex 10, Vol. IV,
    /// §4.3.8.4.2.3.2 defines subfields such as `MTB`, `CVC`, `VRC`, `CHC`,
    /// `HRC`, `VSB`, `HSB`, and `MID`.
    #[serde(rename = "resolution")]
    ResolutionMessage,

    /// `UDS1 = 3`, `UDS2 = 1`.
    ///
    /// Carries an RA broadcast. Annex 10, Vol. IV, §4.3.8.4.2.3.4 defines
    /// `ARA`, `RAC`, `RAT`, `MTE`, `AID`, and `CAC` subfields.
    #[serde(rename = "ra_b")]
    RaBroadcast,

    /// `UDS1 = 3`, `UDS2 = 2`.
    ///
    /// Carries an ACAS broadcast. Annex 10, Vol. IV, §4.3.8.4.2.3.3 defines
    /// `MID` as the 24-bit Mode S address of the interrogating ACAS aircraft.
    #[serde(rename = "acas_b")]
    AcasBroadcast,

    /// Any currently unsupported or unassigned `UDS` combination.
    Unknown,
}

/**
 * Human-readable summary of the UF16 `MU` field.
 *
 * Reference: ICAO Annex 10, Vol. IV, §4.3.8.4.2.3.
 *
 * | Field | Bits | Meaning |
 * | --- | ---: | --- |
 * | `UDS1` | 33-36 | U-definition subfield, high nibble |
 * | `UDS2` | 37-40 | U-definition subfield, low nibble |
 * | `MID` | 65-88 | Mode S address of interrogating ACAS aircraft for resolution messages and ACAS broadcasts |
 */
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AcasMu {
    /// U-definition subfield high nibble, Annex bits 33-36.
    #[serde(skip)]
    pub uds1: u8,

    /// U-definition subfield low nibble, Annex bits 37-40.
    #[serde(skip)]
    pub uds2: u8,

    /// High-level classification derived from (`UDS1`, `UDS2`).
    pub kind: MuKind,

    #[serde(skip_serializing_if = "Option::is_none")]
    /// Mode S address of the interrogating ACAS aircraft, when defined.
    ///
    /// Present for resolution messages and ACAS broadcasts. It occupies Annex
    /// bits 65-88 in the UF16 frame and is formatted as six lowercase
    /// hexadecimal digits, like `DecodedUplink::address`.
    pub mid: Option<String>,
}

/// UF16 — long air-air surveillance interrogation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, DekuRead)]
pub struct Uf16 {
    #[deku(bits = "5")]
    #[serde(skip)]
    /// Uplink format descriptor. For UF16 this is always `16` (`10000`).
    pub uf: u8,

    #[deku(bits = "3")]
    #[serde(skip)]
    /// Spare bits 6-8. Transmitted as zero.
    pub spare0: u8,

    #[deku(bits = "1")]
    /// Reply length command, Annex bit 9.
    pub rl: bool,

    #[deku(bits = "4")]
    #[serde(skip)]
    /// Spare bits 10-13. Transmitted as zero.
    pub spare1: u8,

    #[deku(bits = "1")]
    /// Acquisition bit, Annex bit 14.
    pub aq: bool,

    #[deku(bits = "18")]
    #[serde(skip)]
    /// Spare bits 15-32 in the UF16 top-level layout.
    pub spare2: u32,

    /// ACAS message field, Annex bits 33-88.
    #[serde(skip)]
    pub mu: [u8; 7],

    #[deku(skip, default = "decode_mu_bytes(&mu)")]
    /// Human-readable ACAS `MU` classification and selected subfields.
    pub acas: AcasMu,

    #[deku(bits = "24", endian = "big")]
    #[serde(skip)]
    /// Raw address/parity field, Annex bits 89-112.
    pub ap: u32,
}

pub fn decode(frame: &[u8]) -> Result<Uf16, DekuError> {
    let (_, parsed) = Uf16::from_bytes((frame, 0))?;
    Ok(parsed)
}

fn decode_mu_bytes(mu: &[u8; 7]) -> AcasMu {
    let uds1 = mu[0] >> 4;
    let uds2 = mu[0] & 0x0f;
    let kind = match (uds1, uds2) {
        (3, 0) => MuKind::ResolutionMessage,
        (3, 1) => MuKind::RaBroadcast,
        (3, 2) => MuKind::AcasBroadcast,
        _ => MuKind::Unknown,
    };
    let mid = match kind {
        MuKind::ResolutionMessage | MuKind::AcasBroadcast => {
            let value = ((mu[4] as u32) << 16) | ((mu[5] as u32) << 8) | mu[6] as u32;
            Some(format!("{value:06x}"))
        }
        _ => None,
    };
    AcasMu {
        uds1,
        uds2,
        kind,
        mid,
    }
}

#[allow(dead_code)]
pub fn decode_mu(frame: &[u8]) -> AcasMu {
    let uds1 = extract_bits(frame, 32, 4) as u8;
    let uds2 = extract_bits(frame, 36, 4) as u8;
    let mut mu = [0u8; 7];
    mu.copy_from_slice(&frame[4..11]);
    let mut acas = decode_mu_bytes(&mu);
    acas.uds1 = uds1;
    acas.uds2 = uds2;
    acas
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_layout() {
        let frame = [
            0x80, 0x80, 0x00, 0x00, 0x32, 0x00, 0x00, 0x00, 0x4b, 0x18, 0x04, 0xd2, 0x3f, 0x7c,
        ];
        let parsed = decode(&frame).unwrap();
        assert_eq!(parsed.uf, 16);
        assert!(parsed.rl);
        assert!(!parsed.aq);
        assert_eq!(parsed.mu, [0x32, 0x00, 0x00, 0x00, 0x4b, 0x18, 0x04]);
        assert_eq!(parsed.acas.kind, MuKind::AcasBroadcast);
        assert_eq!(parsed.acas.mid.as_deref(), Some("4b1804"));
        assert_eq!(parsed.ap, 0xd23f7c);
    }
}
