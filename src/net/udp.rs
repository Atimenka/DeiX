//! UDP (RFC 768) поверх IPv4. Используется DNS-клиентом (dns.rs).
//!
//! Модель та же, что у tcp.rs: входящие UDP-кадры складываются в очередь
//! из on_ethernet_frame (вне контекста прерывания, см. rtl8139::poll_deferred),
//! синхронный recv_from разбирает очередь, при простое подчитывая RX-буфер
//! карты напрямую через rtl8139::poll_rx.

use crate::net::ipv4;
use crate::spinlock::SpinLock;
use alloc::collections::VecDeque;
use alloc::vec::Vec;

pub const PROTO_UDP: u8 = 17;
const HEADER_LEN: usize = 8;
const ETH_HLEN: usize = 14;

// Очередь принятых UDP-кадров (полные Ethernet-фреймы).
static RX_QUEUE: SpinLock<VecDeque<Vec<u8>>> = SpinLock::new(VecDeque::new());
const RX_QUEUE_CAP: usize = 32;

pub fn rx_enqueue(frame: Vec<u8>) {
    let mut q = RX_QUEUE.lock();
    if q.len() >= RX_QUEUE_CAP {
        q.pop_front();
    }
    q.push_back(frame);
}

fn rx_dequeue() -> Option<Vec<u8>> {
    RX_QUEUE.lock().pop_front()
}

/// Отправляет UDP-датаграмму. Контрольная сумма UDP = 0 (для IPv4 это
/// легально по RFC 768: «checksum not computed»); целостность пакета
/// дополнительно покрыта контрольной суммой IPv4-заголовка и CRC Ethernet.
pub fn send_to(dst_ip: [u8; 4], dst_port: u16, src_port: u16, payload: &[u8]) -> bool {
    let mut dgram = Vec::with_capacity(HEADER_LEN + payload.len());
    dgram.extend_from_slice(&src_port.to_be_bytes());
    dgram.extend_from_slice(&dst_port.to_be_bytes());
    dgram.extend_from_slice(&((HEADER_LEN + payload.len()) as u16).to_be_bytes());
    dgram.extend_from_slice(&0u16.to_be_bytes());
    dgram.extend_from_slice(payload);
    ipv4::send_packet_resolving(dst_ip, PROTO_UDP, &dgram, 0)
}

/// Разбирает Ethernet-фрейм как UDP: возвращает (src_ip, src_port,
/// dst_port, payload-срез).
fn parse(frame: &[u8]) -> Option<([u8; 4], u16, u16, &[u8])> {
    if frame.len() < ETH_HLEN + 20 + HEADER_LEN {
        return None;
    }
    let ihl = ((frame[ETH_HLEN] & 0x0F) as usize) * 4;
    let udp = ETH_HLEN + ihl;
    if frame.len() < udp + HEADER_LEN {
        return None;
    }
    let mut src_ip = [0u8; 4];
    src_ip.copy_from_slice(&frame[ETH_HLEN + 12..ETH_HLEN + 16]);
    let src_port = u16::from_be_bytes([frame[udp], frame[udp + 1]]);
    let dst_port = u16::from_be_bytes([frame[udp + 2], frame[udp + 3]]);
    let udp_len = u16::from_be_bytes([frame[udp + 4], frame[udp + 5]]) as usize;
    if udp_len < HEADER_LEN {
        return None;
    }
    let data_end = (udp + udp_len).min(frame.len());
    Some((src_ip, src_port, dst_port, &frame[udp + HEADER_LEN..data_end]))
}

/// Синхронно ждёт датаграмму на локальный порт `port` до `timeout_ms`.
/// Возвращает (src_ip, src_port, payload).
pub fn recv_from(port: u16, timeout_ms: u64) -> Option<([u8; 4], u16, Vec<u8>)> {
    let t0 = crate::timer::uptime_ms();
    loop {
        crate::rtl8139::poll_rx();
        while let Some(frame) = rx_dequeue() {
            if let Some((sip, sp, dp, data)) = parse(&frame) {
                if dp == port {
                    return Some((sip, sp, Vec::from(data)));
                }
            }
        }
        if crate::timer::uptime_ms() - t0 > timeout_ms {
            return None;
        }
        unsafe { core::arch::asm!("hlt") };
    }
}
