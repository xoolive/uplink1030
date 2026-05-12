/// Extract up to 32 bits from an MSB-first Mode S byte slice.
///
/// `start_bit` is zero-based: bit 0 is the MSB of `data[0]`.
pub fn extract_bits(data: &[u8], start_bit: usize, num_bits: usize) -> u32 {
    let mut result = 0u32;
    for i in 0..num_bits.min(32) {
        let bit_index = start_bit + i;
        let byte_index = bit_index / 8;
        let bit_in_byte = 7 - (bit_index % 8);
        let bit = (data[byte_index] >> bit_in_byte) & 1;
        result = (result << 1) | bit as u32;
    }
    result
}

pub fn bytes_to_hex(data: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(data.len() * 2);
    for &b in data {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_msb_first_bits() {
        let data = [0b1010_1100, 0b0110_0000];
        assert_eq!(extract_bits(&data, 0, 5), 0b10101);
        assert_eq!(extract_bits(&data, 5, 6), 0b100011);
    }
}
