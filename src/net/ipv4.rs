//! IPv4 (RFC 791): разбор/сборка заголовка, вычисление контрольной суммы,
//! диспетчеризация по протоколу (сейчас поддерживаем только ICMP).

use crate::net::checksum::checksum;
use crate::net::{arp, eth, icmp};
use crate::rtl8139;

pub const PROTO_ICMP: u8 = 1;
const MIN_HEADER_LEN: usize = 20;

pub struct Ipv4Header {
    pub version_ihl: u8,
    pub total_length: u16,
    pub protocol: u8,
    pub src_ip: [u8; 4],
    pub dst_ip: [u8; 4],
    pub header_len: usize,
}

impl Ipv4Header {
    pub fn parse(data: &[u8]) -> Option<Ipv4Header> {
        if data.len() < MIN_HEADER_LEN {
            return None;
        }
        let version_ihl = data[0];
        let ihl = (version_ihl & 0x0F) as usize * 4;
        if data.len() < ihl {
            return None;
        }
        let total_length = u16::from_be_bytes([data[2], data[3]]);
        let protocol = data[9];
        let mut src_ip = [0u8; 4];
        let mut dst_ip = [0u8; 4];
        src_ip.copy_from_slice(&data[12..16]);
        dst_ip.copy_from_slice(&data[16..20]);

        Some(Ipv4Header {
            version_ihl,
            total_length,
            protocol,
            src_ip,
            dst_ip,
            header_len: ihl,
        })
    }
}

pub fn handle_packet(data: &[u8]) {
    let header = match Ipv4Header::parse(data) {
        Some(h) => h,
        None => return,
    };

    let my_ip = crate::net::my_ip();
    if header.dst_ip != my_ip && header.dst_ip != [255, 255, 255, 255] {
        return; // не наш пакет
    }

    let payload_end = (header.total_length as usize).min(data.len());
    if payload_end < header.header_len {
        return;
    }
    let payload = &data[header.header_len..payload_end];

    match header.protocol {
        PROTO_ICMP => icmp::handle_packet(payload, &header),
        _ => {}
    }
}

/// Собирает IPv4-заголовок (20 байт, без опций) в `out`.
fn build_header(protocol: u8, src_ip: [u8; 4], dst_ip: [u8; 4], payload_len: usize, ident: u16, out: &mut [u8]) {
    out[0] = 0x45; // version 4, IHL 5 (20 байт, без опций)
    out[1] = 0; // DSCP/ECN
    let total_len = (MIN_HEADER_LEN + payload_len) as u16;
    out[2..4].copy_from_slice(&total_len.to_be_bytes());
    out[4..6].copy_from_slice(&ident.to_be_bytes());
    out[6..8].copy_from_slice(&0u16.to_be_bytes()); // flags/fragment offset
    out[8] = 64; // TTL
    out[9] = protocol;
    out[10..12].copy_from_slice(&0u16.to_be_bytes()); // checksum (заполним ниже)
    out[12..16].copy_from_slice(&src_ip);
    out[16..20].copy_from_slice(&dst_ip);

    let csum = checksum(&out[0..MIN_HEADER_LEN]);
    out[10..12].copy_from_slice(&csum.to_be_bytes());
}

/// Собирает полный Ethernet+IPv4 фрейм с заданным payload (например, ICMP)
/// и отправляет его через RTL8139. dst_ip должен быть уже разрешён в MAC
/// через arp::resolve (или найден в кэше) — эта функция сама ARP не шлёт.
pub fn send_packet(dst_ip: [u8; 4], dst_mac: [u8; 6], protocol: u8, payload: &[u8], ident: u16) -> bool {
    let my_ip = crate::net::my_ip();
    let my_mac = rtl8139::mac_address();

    let mut ip_packet = [0u8; 1500];
    build_header(protocol, my_ip, dst_ip, payload.len(), ident, &mut ip_packet);
    ip_packet[MIN_HEADER_LEN..MIN_HEADER_LEN + payload.len()].copy_from_slice(payload);
    let ip_packet_len = MIN_HEADER_LEN + payload.len();

    let mut frame = [0u8; 1600];
    let len = eth::build_frame(dst_mac, my_mac, eth::ETHERTYPE_IPV4, &ip_packet[..ip_packet_len], &mut frame);
    rtl8139::send_frame(&frame[..len])
}

/// Удобная обёртка: сама резолвит MAC через ARP (с таймаутом), если нужно.
pub fn send_packet_resolving(dst_ip: [u8; 4], protocol: u8, payload: &[u8], ident: u16) -> bool {
    let gateway = crate::net::gateway_ip();
    let my_ip = crate::net::my_ip();

    // Простейшая логика маршрутизации: если адресат в той же /24 подсети,
    // что и мы — резолвим его напрямую, иначе шлём через шлюз.
    let same_subnet = dst_ip[0] == my_ip[0] && dst_ip[1] == my_ip[1] && dst_ip[2] == my_ip[2];
    let resolve_ip = if same_subnet { dst_ip } else { gateway };

    match arp::resolve(resolve_ip, 2000) {
        Some(mac) => send_packet(dst_ip, mac, protocol, payload, ident),
        None => false,
    }
}
