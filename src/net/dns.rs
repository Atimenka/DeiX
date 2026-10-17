//! DNS-клиент (RFC 1035): настоящий резолвинг A-записей по UDP/53.
//!
//! Запрос уходит на сервер из NetConfig (по умолчанию 10.0.2.3 — DNS,
//! который QEMU slirp предоставляет гостю). Ответы кешируются с учётом
//! TTL записи — по его истечении имя резолвится заново.
//!
//! Ограничения (честно): только IPv4 (QTYPE=A), только UDP (без TCP
//! fallback для ответов >512 байт), без DNSSEC.

use crate::net::udp;
use crate::spinlock::SpinLock;
use crate::sync::without_interrupts;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

const DNS_PORT: u16 = 53;
const ATTEMPTS: u32 = 2;
const ATTEMPT_TIMEOUT_MS: u64 = 2000;
/// TTL кеша ограничиваем сверху 10 минутами, снизу — 5 секундами.
const TTL_MAX_MS: u64 = 600_000;
const TTL_MIN_MS: u64 = 5_000;
const CACHE_CAP: usize = 16;

const MAX_ADDRS: usize = 4;

struct CacheEntry {
    name: String,
    ips: Vec<[u8; 4]>,
    expires_ms: u64,
}

static CACHE: SpinLock<Vec<CacheEntry>> = SpinLock::new(Vec::new());
static QUERY_SEQ: SpinLock<u16> = SpinLock::new(0);

fn cache_get(name: &str) -> Option<Vec<[u8; 4]>> {
    let now = crate::timer::uptime_ms();
    without_interrupts(|| {
        let mut c = CACHE.lock();
        c.retain(|e| e.expires_ms > now);
        c.iter().find(|e| e.name == name).map(|e| e.ips.clone())
    })
}

fn cache_put(name: &str, ips: Vec<[u8; 4]>, ttl_ms: u64) {
    let now = crate::timer::uptime_ms();
    let ttl = ttl_ms.clamp(TTL_MIN_MS, TTL_MAX_MS);
    without_interrupts(|| {
        let mut c = CACHE.lock();
        c.retain(|e| e.name != name);
        if c.len() >= CACHE_CAP {
            c.remove(0);
        }
        c.push(CacheEntry {
            name: name.to_string(),
            ips,
            expires_ms: now + ttl,
        });
    });
}

/// Валидное DNS-имя: метки 1..=63 символов [a-z0-9-_], всего <= 253.
fn valid_hostname(host: &str) -> bool {
    if host.is_empty() || host.len() > 253 {
        return false;
    }
    host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    })
}

/// Собирает DNS-запрос A-записи: заголовок + QNAME + QTYPE/QCLASS.
fn build_query(id: u16, host: &str) -> Vec<u8> {
    let mut q = Vec::with_capacity(17 + host.len());
    q.extend_from_slice(&id.to_be_bytes());
    q.extend_from_slice(&0x0100u16.to_be_bytes()); // flags: RD (recursion desired)
    q.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    q.extend_from_slice(&0u16.to_be_bytes()); // ANCOUNT
    q.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    q.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT
    for label in host.split('.') {
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.push(0); // конец QNAME
    q.extend_from_slice(&1u16.to_be_bytes()); // QTYPE = A
    q.extend_from_slice(&1u16.to_be_bytes()); // QCLASS = IN
    q
}

/// Пропускает NAME в ответе (последовательность меток или compression
/// pointer 0xC0xx, RFC 1035 §4.1.4). Возвращает позицию за именем.
fn skip_name(msg: &[u8], mut pos: usize) -> Option<usize> {
    loop {
        let b = *msg.get(pos)?;
        if b == 0 {
            return Some(pos + 1);
        }
        if b & 0xC0 == 0xC0 {
            return Some(pos + 2);
        }
        pos += 1 + b as usize;
    }
}

/// Разбирает ответ: возвращает все A-записи из секции ответов (не более
/// MAX_ADDRS) и наименьший TTL в мс. CNAME-записи пропускаются — сервер
/// кладёт A каноничного имени в тот же ответ.
fn parse_response(msg: &[u8], expect_id: u16) -> Option<(Vec<[u8; 4]>, u64)> {
    if msg.len() < 12 {
        return None;
    }
    let id = u16::from_be_bytes([msg[0], msg[1]]);
    let flags = u16::from_be_bytes([msg[2], msg[3]]);
    if id != expect_id || flags & 0x8000 == 0 {
        return None; // не наш ответ или это не ответ вовсе
    }
    if flags & 0x000F != 0 {
        return None; // RCODE != 0: NXDOMAIN, SERVFAIL и т.д.
    }
    let qdcount = u16::from_be_bytes([msg[4], msg[5]]) as usize;
    let ancount = u16::from_be_bytes([msg[6], msg[7]]) as usize;

    let mut pos = 12;
    for _ in 0..qdcount {
        pos = skip_name(msg, pos)? + 4; // QTYPE + QCLASS
    }

    let mut ips: Vec<[u8; 4]> = Vec::new();
    let mut min_ttl_ms = u64::MAX;
    for _ in 0..ancount {
        pos = skip_name(msg, pos)?;
        if pos + 10 > msg.len() {
            return None;
        }
        let rtype = u16::from_be_bytes([msg[pos], msg[pos + 1]]);
        let rclass = u16::from_be_bytes([msg[pos + 2], msg[pos + 3]]);
        let ttl = u32::from_be_bytes([msg[pos + 4], msg[pos + 5], msg[pos + 6], msg[pos + 7]]);
        let rdlen = u16::from_be_bytes([msg[pos + 8], msg[pos + 9]]) as usize;
        pos += 10;
        if pos + rdlen > msg.len() {
            return None;
        }
        if rtype == 1 && rclass == 1 && rdlen == 4 {
            let ip = [msg[pos], msg[pos + 1], msg[pos + 2], msg[pos + 3]];
            if ips.len() < MAX_ADDRS && !ips.contains(&ip) {
                ips.push(ip);
            }
            min_ttl_ms = min_ttl_ms.min(ttl as u64 * 1000);
        }
        pos += rdlen; // CNAME и прочие типы пропускаем
    }
    if ips.is_empty() { None } else { Some((ips, min_ttl_ms)) }
}

/// Резолвит первую A-запись `host`.
pub fn resolve_a(host: &str) -> Option<[u8; 4]> {
    resolve_all(host).first().copied()
}

/// Резолвит все A-записи `host` (до MAX_ADDRS) через настроенный
/// DNS-сервер. Пустой список — имя не разрешилось (нет сети, таймаут
/// или NXDOMAIN).
pub fn resolve_all(host: &str) -> Vec<[u8; 4]> {
    let name = host.trim().to_lowercase();
    if !valid_hostname(&name) {
        return Vec::new();
    }
    if let Some(ips) = cache_get(&name) {
        return ips;
    }
    if !crate::rtl8139::is_ready() {
        return Vec::new();
    }

    let server = crate::net::dns_ip();
    for _ in 0..ATTEMPTS {
        let seq = without_interrupts(|| {
            let mut s = QUERY_SEQ.lock();
            *s = s.wrapping_add(1);
            *s
        });
        // ID из тиков таймера + счётчика, локальный порт из
        // эфемерального диапазона — защита от случайных совпадений.
        let id = (crate::timer::uptime_ms() as u16) ^ (seq.rotate_left(7)) ^ 0x5A17;
        let sport = 49152 + (seq & 0x3FFF);
        let query = build_query(id, &name);
        if !udp::send_to(server, DNS_PORT, sport, &query) {
            continue;
        }

        let deadline = crate::timer::uptime_ms() + ATTEMPT_TIMEOUT_MS;
        loop {
            let left = deadline.saturating_sub(crate::timer::uptime_ms());
            if left == 0 {
                break;
            }
            match udp::recv_from(sport, left) {
                Some((sip, sp, data)) => {
                    if sip != server || sp != DNS_PORT {
                        continue; // чужая датаграмма — ждём дальше
                    }
                    if let Some((ips, ttl_ms)) = parse_response(&data, id) {
                        cache_put(&name, ips.clone(), ttl_ms);
                        return ips;
                    }
                    // Ответ сервера с ошибкой (NXDOMAIN и т.п.) —
                    // повторять бессмысленно.
                    return Vec::new();
                }
                None => break, // таймаут попытки
            }
        }
    }
    Vec::new()
}

/// Сбрасывает DNS-кеш (используется при смене сетевой конфигурации).
pub fn flush_cache() {
    without_interrupts(|| CACHE.lock().clear());
}
