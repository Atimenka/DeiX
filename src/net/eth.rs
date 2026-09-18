//! Разбор и сборка заголовков Ethernet II.

pub const HEADER_LEN: usize = 14;
pub const ETHERTYPE_IPV4: u16 = 0x0800;
pub const ETHERTYPE_ARP: u16 = 0x0806;

pub const BROADCAST_MAC: [u8; 6] = [0xFF; 6];

pub struct EthernetHeader {
    pub dst_mac: [u8; 6],
    pub src_mac: [u8; 6],
    pub ethertype: u16,
}

impl EthernetHeader {
    pub fn parse(frame: &[u8]) -> Option<EthernetHeader> {
        if frame.len() < HEADER_LEN {
            return None;
        }
        let mut dst_mac = [0u8; 6];
        let mut src_mac = [0u8; 6];
        dst_mac.copy_from_slice(&frame[0..6]);
        src_mac.copy_from_slice(&frame[6..12]);
        let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
        Some(EthernetHeader {
            dst_mac,
            src_mac,
            ethertype,
        })
    }
}

/// Собирает Ethernet-фрейм: заголовок + payload в один буфер, который
/// можно сразу передать в rtl8139::send_frame.
pub fn build_frame(dst_mac: [u8; 6], src_mac: [u8; 6], ethertype: u16, payload: &[u8], out: &mut [u8]) -> usize {
    out[0..6].copy_from_slice(&dst_mac);
    out[6..12].copy_from_slice(&src_mac);
    out[12..14].copy_from_slice(&ethertype.to_be_bytes());
    out[14..14 + payload.len()].copy_from_slice(payload);
    HEADER_LEN + payload.len()
}
