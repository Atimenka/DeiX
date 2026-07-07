//! ARP (Address Resolution Protocol, RFC 826): сопоставление IP-адресов
//! с MAC-адресами. Реализовано минимально: отвечаем на входящие ARP-запросы
//! (кто-то спрашивает наш MAC) и умеем сами разослать ARP-запрос и
//! дождаться ответа (нужно, чтобы узнать MAC шлюза перед отправкой ping).

use crate::net::eth::{self, EthernetHeader};
use crate::rtl8139;
use crate::spinlock::SpinLock;
use crate::sync::without_interrupts;
use crate::timer;

const ARP_HTYPE_ETHERNET: u16 = 1;
const ARP_PTYPE_IPV4: u16 = 0x0800;
const ARP_OP_REQUEST: u16 = 1;
const ARP_OP_REPLY: u16 = 2;
const ARP_PACKET_LEN: usize = 28;

const ARP_CACHE_SIZE: usize = 8;

#[derive(Clone, Copy)]
struct ArpEntry {
    ip: [u8; 4],
    mac: [u8; 6],
    valid: bool,
}

struct ArpCache {
    entries: [ArpEntry; ARP_CACHE_SIZE],
    next_slot: usize,
}

impl ArpCache {
    const fn new() -> Self {
        ArpCache {
            entries: [ArpEntry {
                ip: [0; 4],
                mac: [0; 6],
                valid: false,
            }; ARP_CACHE_SIZE],
            next_slot: 0,
        }
    }

    fn insert(&mut self, ip: [u8; 4], mac: [u8; 6]) {
        for entry in self.entries.iter_mut() {
            if entry.valid && entry.ip == ip {
                entry.mac = mac;
                return;
            }
        }
        let slot = self.next_slot;
        self.entries[slot] = ArpEntry { ip, mac, valid: true };
        self.next_slot = (self.next_slot + 1) % ARP_CACHE_SIZE;
    }

    fn lookup(&self, ip: [u8; 4]) -> Option<[u8; 6]> {
        self.entries
            .iter()
            .find(|e| e.valid && e.ip == ip)
            .map(|e| e.mac)
    }
}

static CACHE: SpinLock<ArpCache> = SpinLock::new(ArpCache::new());

pub fn cache_lookup(ip: [u8; 4]) -> Option<[u8; 6]> {
    without_interrupts(|| CACHE.lock().lookup(ip))
}

pub fn cache_insert(ip: [u8; 4], mac: [u8; 6]) {
    without_interrupts(|| CACHE.lock().insert(ip, mac));
}

/// Печатает содержимое ARP-кэша (для команды CLI `arp`).
pub fn print_cache() {
    without_interrupts(|| {
        let cache = CACHE.lock();
        let mut any = false;
        for entry in cache.entries.iter() {
            if entry.valid {
                any = true;
                crate::println!(
                    "  {}.{}.{}.{} -> {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                    entry.ip[0], entry.ip[1], entry.ip[2], entry.ip[3],
                    entry.mac[0], entry.mac[1], entry.mac[2], entry.mac[3], entry.mac[4], entry.mac[5]
                );
            }
        }
        if !any {
            crate::println!("  (empty)");
        }
    });
}

fn build_arp_packet(op: u16, src_mac: [u8; 6], src_ip: [u8; 4], dst_mac: [u8; 6], dst_ip: [u8; 4], out: &mut [u8]) {
    out[0..2].copy_from_slice(&ARP_HTYPE_ETHERNET.to_be_bytes());
    out[2..4].copy_from_slice(&ARP_PTYPE_IPV4.to_be_bytes());
    out[4] = 6; // hardware address length
    out[5] = 4; // protocol address length
    out[6..8].copy_from_slice(&op.to_be_bytes());
    out[8..14].copy_from_slice(&src_mac);
    out[14..18].copy_from_slice(&src_ip);
    out[18..24].copy_from_slice(&dst_mac);
    out[24..28].copy_from_slice(&dst_ip);
}

/// Обрабатывает входящий ARP-пакет: если это запрос на наш IP — отвечаем;
/// в любом случае запоминаем отправителя в кэше (пассивное обучение, как
/// делают почти все стеки).
pub fn handle_packet(payload: &[u8], eth_header: &EthernetHeader) {
    if payload.len() < ARP_PACKET_LEN {
        return;
    }

    let op = u16::from_be_bytes([payload[6], payload[7]]);
    let mut sender_mac = [0u8; 6];
    sender_mac.copy_from_slice(&payload[8..14]);
    let mut sender_ip = [0u8; 4];
    sender_ip.copy_from_slice(&payload[14..18]);
    let mut target_ip = [0u8; 4];
    target_ip.copy_from_slice(&payload[24..28]);

    cache_insert(sender_ip, sender_mac);

    let my_ip = crate::net::my_ip();
    if op == ARP_OP_REQUEST && target_ip == my_ip {
        let my_mac = rtl8139::mac_address();

        let mut arp_reply = [0u8; ARP_PACKET_LEN];
        build_arp_packet(ARP_OP_REPLY, my_mac, my_ip, sender_mac, sender_ip, &mut arp_reply);

        let mut frame = [0u8; eth::HEADER_LEN + ARP_PACKET_LEN];
        let len = eth::build_frame(eth_header.src_mac, my_mac, eth::ETHERTYPE_ARP, &arp_reply, &mut frame);
        rtl8139::send_frame(&frame[..len]);
    }
}

/// Рассылает ARP-запрос "кто владеет этим IP?" и синхронно (крутясь в hlt)
/// ждёт ответа до истечения таймаута. Используется перед отправкой ping,
/// если MAC адресата ещё не в кэше.
pub fn resolve(ip: [u8; 4], timeout_ms: u64) -> Option<[u8; 6]> {
    if let Some(mac) = cache_lookup(ip) {
        return Some(mac);
    }

    let my_mac = rtl8139::mac_address();
    let my_ip = crate::net::my_ip();

    let mut arp_request = [0u8; ARP_PACKET_LEN];
    build_arp_packet(ARP_OP_REQUEST, my_mac, my_ip, [0; 6], ip, &mut arp_request);

    let mut frame = [0u8; eth::HEADER_LEN + ARP_PACKET_LEN];
    let len = eth::build_frame(eth::BROADCAST_MAC, my_mac, eth::ETHERTYPE_ARP, &arp_request, &mut frame);
    rtl8139::send_frame(&frame[..len]);

    let start = timer::uptime_ms();
    loop {
        if let Some(mac) = cache_lookup(ip) {
            return Some(mac);
        }
        if timer::uptime_ms() - start > timeout_ms {
            return None;
        }
        unsafe { core::arch::asm!("hlt") };
    }
}
