use std::sync::OnceLock;

const CRC24_GENERATOR: u32 = 0xFFF409;
const CRC24_GENERATOR_FULL: u64 = 0x1FFF409; // includes x^24 term

pub fn crc24(data: &[u8], n_bits: usize) -> u32 {
    let mut crc = 0u32;
    for i in 0..n_bits {
        let byte_idx = i / 8;
        let bit_idx = 7 - (i % 8);
        let bit = ((data[byte_idx] >> bit_idx) & 1) as u32;
        let msb = (crc >> 23) & 1;
        crc = ((crc << 1) | bit) & 0xFF_FFFF;
        if msb != 0 {
            crc ^= CRC24_GENERATOR;
        }
    }
    crc
}

pub fn ap_overlay_from_address(address: u32) -> u32 {
    let address = address & 0xFF_FFFF;
    let mut product = 0u64;
    for i in 0..24 {
        if (address & (1 << (23 - i))) != 0 {
            product ^= CRC24_GENERATOR_FULL << (23 - i);
        }
    }
    ((product >> 24) as u32) & 0xFF_FFFF
}

pub fn ap_address_from_overlay(overlay_bits: u32) -> u32 {
    let inverse_rows = inverse_rows();
    let mut address = 0u32;
    for (row, mask) in inverse_rows.iter().copied().enumerate() {
        if ((mask & overlay_bits).count_ones() & 1) != 0 {
            address |= 1 << (23 - row);
        }
    }
    address & 0xFF_FFFF
}

pub fn recover_ap_address(msg: &[u8], n_bits: usize) -> u32 {
    let parity_start = (n_bits - 24) / 8;
    let parity = ((msg[parity_start] as u32) << 16)
        | ((msg[parity_start + 1] as u32) << 8)
        | msg[parity_start + 2] as u32;
    let crc = crc24(msg, n_bits - 24);
    ap_address_from_overlay(crc ^ parity)
}

fn inverse_rows() -> &'static [u32; 24] {
    static ROWS: OnceLock<[u32; 24]> = OnceLock::new();
    ROWS.get_or_init(build_overlay_inverse)
}

fn build_overlay_inverse() -> [u32; 24] {
    let mut mat = [0u32; 24];
    let mut inv = [0u32; 24];

    for i in 0..24 {
        let col = ap_overlay_from_address(1 << (23 - i));
        for j in 0..24 {
            if (col & (1 << (23 - j))) != 0 {
                mat[j] |= 1 << (23 - i);
            }
        }
    }
    for (i, v) in inv.iter_mut().enumerate() {
        *v = 1 << (23 - i);
    }

    for col in 0..24 {
        let mask = 1 << (23 - col);
        let pivot = (col..24)
            .find(|&row| (mat[row] & mask) != 0)
            .expect("uplink AP overlay matrix should be invertible");
        if pivot != col {
            mat.swap(pivot, col);
            inv.swap(pivot, col);
        }
        for row in 0..24 {
            if row != col && (mat[row] & mask) != 0 {
                mat[row] ^= mat[col];
                inv[row] ^= inv[col];
            }
        }
    }

    inv
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_round_trip() {
        for addr in [0, 1, 0x399098, 0xabcdef, 0xffffff] {
            let overlay = ap_overlay_from_address(addr);
            assert_eq!(ap_address_from_overlay(overlay), addr & 0xFF_FFFF);
        }
    }
}
