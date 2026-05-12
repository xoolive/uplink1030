use serde::Serialize;

use crate::bits::bytes_to_hex;
use crate::crc::recover_ap_address;
use crate::decode::acas::{decode_mu, AcasMu};
use crate::decode::deku_uplink::{parse_uf0, parse_uf16};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UplinkFormat {
    Uf0,
    Uf4,
    Uf5,
    Uf11,
    Uf16,
    Uf20,
    Uf21,
    Uf24,
    Unknown(u8),
}

impl UplinkFormat {
    pub fn from_frame(data: &[u8]) -> Self {
        let first_five = data[0] >> 3;
        match first_five {
            0 => Self::Uf0,
            4 => Self::Uf4,
            5 => Self::Uf5,
            11 => Self::Uf11,
            16 => Self::Uf16,
            20 => Self::Uf20,
            21 => Self::Uf21,
            _ if (data[0] & 0xC0) == 0xC0 => Self::Uf24,
            other => Self::Unknown(other),
        }
    }

    pub fn number(self) -> u8 {
        match self {
            Self::Uf0 => 0,
            Self::Uf4 => 4,
            Self::Uf5 => 5,
            Self::Uf11 => 11,
            Self::Uf16 => 16,
            Self::Uf20 => 20,
            Self::Uf21 => 21,
            Self::Uf24 => 24,
            Self::Unknown(v) => v,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Uf0Fields {
    pub rl: u8,
    pub aq: u8,
    pub ds: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Uf16Fields {
    pub rl: u8,
    pub aq: u8,
    pub mu: [u8; 7],
    pub acas: AcasMu,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DecodedFields {
    Uf0(Uf0Fields),
    Uf16(Uf16Fields),
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DecodedUplink {
    pub raw: String,
    pub uf: u8,
    pub bits: usize,
    pub address: String,
    pub address_u32: u32,
    pub fields: DecodedFields,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    InvalidLength(usize),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidLength(len) => write!(f, "invalid Mode S uplink byte length: {len}"),
        }
    }
}

impl std::error::Error for DecodeError {}

pub fn decode_frame(frame: &[u8]) -> Result<DecodedUplink, DecodeError> {
    let bits = match frame.len() {
        7 => 56,
        14 => 112,
        len => return Err(DecodeError::InvalidLength(len)),
    };
    let uf = UplinkFormat::from_frame(frame);
    let address_u32 = recover_ap_address(frame, bits);
    let fields = match uf {
        UplinkFormat::Uf0 if bits == 56 => {
            let raw = parse_uf0(frame).expect("UF0 layout should parse after length/UF checks");
            DecodedFields::Uf0(Uf0Fields {
                rl: raw.rl,
                aq: raw.aq,
                ds: raw.ds,
            })
        }
        UplinkFormat::Uf16 if bits == 112 => {
            let raw = parse_uf16(frame).expect("UF16 layout should parse after length/UF checks");
            DecodedFields::Uf16(Uf16Fields {
                rl: raw.rl,
                aq: raw.aq,
                mu: raw.mu,
                acas: decode_mu(frame),
            })
        }
        _ => DecodedFields::Unsupported,
    };

    Ok(DecodedUplink {
        raw: bytes_to_hex(frame),
        uf: uf.number(),
        bits,
        address: format!("{address_u32:06x}"),
        address_u32,
        fields,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_known_uf0() {
        let frame = [0x00, 0x00, 0x00, 0x00, 0x72, 0x19, 0x51];
        let decoded = decode_frame(&frame).unwrap();
        assert_eq!(decoded.uf, 0);
        assert_eq!(decoded.address, "4b1618");
    }
}
