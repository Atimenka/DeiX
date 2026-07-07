//! Сетевой стек мини-ОС: Ethernet -> ARP / IPv4 -> ICMP.
//!
//! Всё максимально просто и синхронно (без сокетов, без очередей запросов):
//! пакеты приходят через прерывание карты (см. rtl8139.rs::on_interrupt),
//! разбираются здесь и сразу обрабатываются (ARP-ответ на ARP-запрос,
//! ICMP-эхо на ping и т.д.). Для одиночной учебной ОС этого достаточно.

pub mod arp;
pub mod checksum;
pub mod eth;
pub mod icmp;
pub mod ipv4;

use crate::spinlock::SpinLock;
use crate::sync::without_interrupts;

/// Конфигурация сетевого интерфейса: наш IP-адрес (по умолчанию — это
/// фиксированный адрес, который QEMU user-mode networking (slirp) выдаёт
/// гостю "из коробки": 10.0.2.15/24, шлюз/хост 10.0.2.2). Меняется командой
/// `ifconfig <ip>` в CLI, если понадобится другая сеть (например, при
/// `-netdev tap` с другой подсетью).
pub struct NetConfig {
    pub ip: [u8; 4],
    pub gateway: [u8; 4],
}

impl NetConfig {
    const fn default() -> Self {
        NetConfig {
            ip: [10, 0, 2, 15],
            gateway: [10, 0, 2, 2],
        }
    }
}

static CONFIG: SpinLock<NetConfig> = SpinLock::new(NetConfig::default());

pub fn my_ip() -> [u8; 4] {
    without_interrupts(|| CONFIG.lock().ip)
}

pub fn gateway_ip() -> [u8; 4] {
    without_interrupts(|| CONFIG.lock().gateway)
}

pub fn set_my_ip(ip: [u8; 4]) {
    without_interrupts(|| CONFIG.lock().ip = ip);
}

/// Вызывается драйвером карты (rtl8139.rs) для каждого принятого
/// Ethernet-фрейма. Работает в контексте обработчика прерывания.
pub fn on_ethernet_frame(frame: &[u8]) {
    let header = match eth::EthernetHeader::parse(frame) {
        Some(h) => h,
        None => return,
    };

    let payload = &frame[eth::HEADER_LEN..];

    match header.ethertype {
        eth::ETHERTYPE_ARP => arp::handle_packet(payload, &header),
        eth::ETHERTYPE_IPV4 => ipv4::handle_packet(payload),
        _ => {}
    }
}
