use serde::Serialize;

use crate::bits::extract_bits;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MuKind {
    ResolutionMessage,
    RaBroadcast,
    AcasBroadcast,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AcasMu {
    pub uds1: u8,
    pub uds2: u8,
    pub kind: MuKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mid: Option<u32>,
}

pub fn decode_mu(frame: &[u8]) -> AcasMu {
    // MU occupies bits 33-88 in Annex numbering, i.e. zero-based 32..88.
    let uds1 = extract_bits(frame, 32, 4) as u8;
    let uds2 = extract_bits(frame, 36, 4) as u8;
    let kind = match (uds1, uds2) {
        (3, 0) => MuKind::ResolutionMessage,
        (3, 1) => MuKind::RaBroadcast,
        (3, 2) => MuKind::AcasBroadcast,
        _ => MuKind::Unknown,
    };
    let mid = match kind {
        MuKind::ResolutionMessage | MuKind::AcasBroadcast => Some(extract_bits(frame, 64, 24)),
        _ => None,
    };
    AcasMu {
        uds1,
        uds2,
        kind,
        mid,
    }
}
