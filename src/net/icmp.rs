//! ICMP (RFC 792): обрабатываем входящий Echo Request (отвечаем Echo Reply,
//! чтобы нас можно было пинговать снаружи) и умеем сами слать Echo Request
//! + ждать Echo Reply для команды CLI `ping`.

use crate::net::checksum::checksum;
use crate::net::ipv4::{self, Ipv4Header};
use crate::spinlock::SpinLock;
use crate::sync::without_interrupts;
use crate::timer;

const TYPE_ECHO_REPLY: u8 = 0;
const TYPE_ECHO_REQUEST: u8 = 8;
const ICMP_HEADER_LEN: usize = 8;

/// Запоминаем время последнего полученного Echo Reply с нужным id/seq —
/// простейший способ синхронно дождаться ответа на исходящий ping без
/// полноценной очереди сообщений.
struct PingState {
    waiting_id: u16,
    waiting_seq: u16,
    reply_received: bool,
}

static PING_STATE: SpinLock<PingState> = SpinLock::new(PingState {
    waiting_id: 0,
    waiting_seq: 0,
    reply_received: false,
});

pub fn handle_packet(data: &[u8], ip_header: &Ipv4Header) {
    if data.len() < ICMP_HEADER_LEN {
        return;
    }

    let icmp_type = data[0];

    match icmp_type {
        TYPE_ECHO_REQUEST => {
            // Отвечаем Echo Reply с тем же id/seq/данными — просто меняем
            // type на 0 и пересчитываем чек-сумму.
            let mut reply = [0u8; 1500];
            let len = data.len().min(1500);
            reply[..len].copy_from_slice(&data[..len]);
            reply[0] = TYPE_ECHO_REPLY;
            reply[2..4].copy_from_slice(&0u16.to_be_bytes());
            let csum = checksum(&reply[..len]);
            reply[2..4].copy_from_slice(&csum.to_be_bytes());

            ipv4::send_packet(ip_header.src_ip, crate::net::arp::cache_lookup(ip_header.src_ip).unwrap_or([0xff; 6]), ipv4::PROTO_ICMP, &reply[..len], 0);
        }
        TYPE_ECHO_REPLY => {
            if data.len() < ICMP_HEADER_LEN {
                return;
            }
            let id = u16::from_be_bytes([data[4], data[5]]);
            let seq = u16::from_be_bytes([data[6], data[7]]);

            without_interrupts(|| {
                let mut state = PING_STATE.lock();
                if state.waiting_id == id && state.waiting_seq == seq && !state.reply_received {
                    state.reply_received = true;
                }
            });
        }
        _ => {}
    }
}

fn build_echo_request(id: u16, seq: u16, payload: &[u8], out: &mut [u8]) -> usize {
    out[0] = TYPE_ECHO_REQUEST;
    out[1] = 0; // code
    out[2..4].copy_from_slice(&0u16.to_be_bytes()); // checksum, заполним ниже
    out[4..6].copy_from_slice(&id.to_be_bytes());
    out[6..8].copy_from_slice(&seq.to_be_bytes());
    let total_len = ICMP_HEADER_LEN + payload.len();
    out[8..total_len].copy_from_slice(payload);

    let csum = checksum(&out[..total_len]);
    out[2..4].copy_from_slice(&csum.to_be_bytes());
    total_len
}

/// Отправляет один ICMP Echo Request на dst_ip и ждёт ответа не дольше
/// timeout_ms. Возвращает время round-trip в миллисекундах, если ответ
/// пришёл вовремя.
pub fn ping(dst_ip: [u8; 4], id: u16, seq: u16, timeout_ms: u64) -> Option<u64> {
    without_interrupts(|| {
        let mut state = PING_STATE.lock();
        state.waiting_id = id;
        state.waiting_seq = seq;
        state.reply_received = false;
    });

    let payload = b"deix-ping-0123456789";
    let mut icmp_packet = [0u8; ICMP_HEADER_LEN + 32];
    let len = build_echo_request(id, seq, payload, &mut icmp_packet);

    let start = timer::uptime_ms();

    if !ipv4::send_packet_resolving(dst_ip, ipv4::PROTO_ICMP, &icmp_packet[..len], id) {
        return None;
    }

    loop {
        let received = without_interrupts(|| PING_STATE.lock().reply_received);
        if received {
            return Some(timer::uptime_ms() - start);
        }
        let elapsed = timer::uptime_ms() - start;
        if elapsed > timeout_ms {
            return None;
        }
        unsafe { core::arch::asm!("hlt") };
    }
}
