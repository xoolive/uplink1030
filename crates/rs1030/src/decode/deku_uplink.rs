use deku::prelude::*;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, DekuRead)]
pub struct RawUf0 {
    #[deku(bits = "5")]
    pub uf: u8,
    #[deku(bits = "3")]
    #[serde(skip)]
    pub spare0: u8,
    #[deku(bits = "1")]
    pub rl: u8,
    #[deku(bits = "4")]
    #[serde(skip)]
    pub spare1: u8,
    #[deku(bits = "1")]
    pub aq: u8,
    pub ds: u8,
    #[deku(bits = "10")]
    #[serde(skip)]
    pub spare2: u16,
    #[deku(bits = "24", endian = "big")]
    pub ap: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, DekuRead)]
pub struct RawUf16 {
    #[deku(bits = "5")]
    pub uf: u8,
    #[deku(bits = "3")]
    #[serde(skip)]
    pub spare0: u8,
    #[deku(bits = "1")]
    pub rl: u8,
    #[deku(bits = "4")]
    #[serde(skip)]
    pub spare1: u8,
    #[deku(bits = "1")]
    pub aq: u8,
    #[deku(bits = "18")]
    #[serde(skip)]
    pub spare2: u32,
    pub mu: [u8; 7],
    #[deku(bits = "24", endian = "big")]
    pub ap: u32,
}

pub fn parse_uf0(frame: &[u8]) -> Result<RawUf0, DekuError> {
    let (_, parsed) = RawUf0::from_bytes((frame, 0))?;
    Ok(parsed)
}

pub fn parse_uf16(frame: &[u8]) -> Result<RawUf16, DekuError> {
    let (_, parsed) = RawUf16::from_bytes((frame, 0))?;
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_uf0_layout() {
        let frame = [0x00, 0x00, 0x00, 0x00, 0x72, 0x19, 0x51];
        let parsed = parse_uf0(&frame).unwrap();
        assert_eq!(parsed.uf, 0);
        assert_eq!(parsed.rl, 0);
        assert_eq!(parsed.aq, 0);
        assert_eq!(parsed.ds, 0);
        assert_eq!(parsed.ap, 0x721951);
    }

    #[test]
    fn parses_uf16_layout() {
        let frame = [
            0x80, 0x80, 0x00, 0x00, 0x32, 0x00, 0x00, 0x00, 0x4b, 0x18, 0x04, 0xd2, 0x3f, 0x7c,
        ];
        let parsed = parse_uf16(&frame).unwrap();
        assert_eq!(parsed.uf, 16);
        assert_eq!(parsed.rl, 1);
        assert_eq!(parsed.aq, 0);
        assert_eq!(parsed.mu, [0x32, 0x00, 0x00, 0x00, 0x4b, 0x18, 0x04]);
        assert_eq!(parsed.ap, 0xd23f7c);
    }
}
