//! Стандартная интернет-контрольная сумма (RFC 1071), используется и в
//! заголовке IPv4, и в ICMP.

pub fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut chunks = data.chunks_exact(2);

    for chunk in &mut chunks {
        let word = u16::from_be_bytes([chunk[0], chunk[1]]);
        sum += word as u32;
    }

    if let [last] = chunks.remainder() {
        sum += (*last as u32) << 8;
    }

    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    !(sum as u16)
}
