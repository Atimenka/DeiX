//! DNS-клиент (RFC 1035): настоящий резолвинг A-записей по UDP/53.
//!
//! Запрос уходит на сервер из NetConfig (по умолчанию 10.0.2.3 — DNS,
//! который QEMU slirp предоставляет гостю). Если настроенный сервер не
//! отвечает (случается в WSL/за NAT, где slirp-DNS не может достучаться
//! до хостового резолвера), запрос честно повторяется на публичные
//! 1.1.1.1 и 8.8.8.8 — через slirp они достижимы обычным NAT. Ответы
//! с флагом TC (обрезание >512 байт) и полный отказ UDP повторяются
//! по TCP/53 (RFC 1035 §4.2.2). Ответы кешируются с учётом TTL записи —
//! по его истечении имя резолвится заново.
//!
//! Ограничения (честно): только IPv4 (QTYPE=A), без DNSSEC.

use crate::net::udp;
use crate::spinlock::SpinLock;
use crate::sync::without_interrupts;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

const DNS_PORT: u16 = 53;
/// UDP-попыток на основной сервер и на каждый резервный.
const ATTEMPTS_PRIMARY: u32 = 2;
const ATTEMPTS_FALLBACK: u32 = 1;
const ATTEMPT_TIMEOUT_MS: u64 = 2000;
/// Общий таймаут TCP/53-запроса (RFC 1035 §4.2.2: TCP обязателен при TC).
const TCP_TIMEOUT_MS: u64 = 3000;
/// Резервные публичные серверы: используются, только когда настроенный
/// (NetConfig::dns) не ответил вовсе — NXDOMAIN с него остаётся окончательным.
const FALLBACK_SERVERS: [[u8; 4]; 2] = [[1, 1, 1, 1], [8, 8, 8, 8]];
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
static LAST_ERROR: SpinLock<String> = SpinLock::new(String::new());
static QUERY_SEQ: SpinLock<u16> = SpinLock::new(0);

fn cache_get(name: &str) -> Option<Vec<[u8; 4]>> {
    let now = crate::timer::uptime_ms();
    without_interrupts(|| {
        let c = CACHE.lock();
        c.iter().find(|e| e.name == name && e.expires_ms > now).map(|e| e.ips.clone())
    })
}

/// Запись с истёкшим TTL: используется, только если свежий запрос не
/// получил ответа (RFC 8767, serve-stale).
fn cache_stale(name: &str) -> Option<Vec<[u8; 4]>> {
    without_interrupts(|| {
        let c = CACHE.lock();
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

/// Результат разбора ответа DNS.
enum Parsed {
    /// Есть A-записи и наименьший TTL в мс.
    Answer(Vec<[u8; 4]>, u64),
    /// Флаг TC: ответ обрезан, нужен повтор по TCP/53.
    Truncated,
    /// Сервер ответил ошибкой (RCODE) или в ответе нет A-записей.
    Reject(String),
}

/// Разбирает ответ: все A-записи из секции ответов (не более MAX_ADDRS)
/// и наименьший TTL в мс. CNAME-записи пропускаются — сервер кладёт A
/// каноничного имени в тот же ответ.
fn parse_response(msg: &[u8], expect_id: u16) -> Option<Parsed> {
    if msg.len() < 12 {
        return None;
    }
    let id = u16::from_be_bytes([msg[0], msg[1]]);
    let flags = u16::from_be_bytes([msg[2], msg[3]]);
    if id != expect_id || flags & 0x8000 == 0 {
        return None; // не наш ответ или это не ответ вовсе
    }
    if flags & 0x000F != 0 {
        let rcode = flags & 0x000F;
        let why = match rcode {
            3 => String::from("имя не существует (NXDOMAIN)"),
            2 => String::from("SERVFAIL: сервер не смог обработать запрос"),
            5 => String::from("запрос отклонён (REFUSED)"),
            c => format!("ошибка сервера (RCODE {})", c),
        };
        return Some(Parsed::Reject(why));
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
    if ips.is_empty() {
        if flags & 0x0200 != 0 {
            return Some(Parsed::Truncated); // TC: A-записи не влезли в UDP-ответ
        }
        return Some(Parsed::Reject(String::from("ответ без A-записей")));
    }
    Some(Parsed::Answer(ips, min_ttl_ms))
}

fn set_error(msg: String) {
    without_interrupts(|| *LAST_ERROR.lock() = msg);
}

/// Причина последней неудачи резолвинга.
pub fn last_error() -> String {
    without_interrupts(|| (*LAST_ERROR.lock()).clone())
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
        set_error(format!("недопустимое имя '{}'", name));
        return Vec::new();
    }
    if let Some(ips) = cache_get(&name) {
        return ips;
    }
    if !crate::rtl8139::is_ready() {
        set_error(String::from("сетевая карта не готова"));
        return Vec::new();
    }

    let servers = server_list();
    let mut report = String::new();
    for (si, server) in servers.iter().enumerate() {
        let seq = without_interrupts(|| {
            let mut s = QUERY_SEQ.lock();
            *s = s.wrapping_add(1);
            *s
        });
        // ID из тиков таймера + счётчика, локальный порт из эфемерального
        // диапазона. Один ID и порт на все попытки одному серверу: поздний
        // ответ на первую отправку принимается так же, как на повторную.
        let id = (crate::timer::uptime_ms() as u16) ^ (seq.rotate_left(7)) ^ 0x5A17;
        let sport = 49152 + (seq & 0x3FFF);
        let query = build_query(id, &name);
        let attempts = if si == 0 { ATTEMPTS_PRIMARY } else { ATTEMPTS_FALLBACK };

        let mut outcome: Option<Parsed> = None;
        for _ in 0..attempts {
            if !udp::send_to(*server, DNS_PORT, sport, &query) {
                continue;
            }
            let deadline = crate::timer::uptime_ms() + ATTEMPT_TIMEOUT_MS;
            loop {
                let left = deadline.saturating_sub(crate::timer::uptime_ms());
                if left == 0 {
                    break;
                }
                let Some((sip, sp, data)) = udp::recv_from(sport, left) else { break };
                if sip != *server || sp != DNS_PORT || data.len() < 2
                    || u16::from_be_bytes([data[0], data[1]]) != id
                {
                    continue;
                }
                if let Some(parsed) = parse_response(&data, id) {
                    outcome = Some(parsed);
                    break;
                }
                // Пакет наш по порту, но не разобрался (обрывок/мусор) —
                // ждём следующий до конца таймаута попытки.
            }
            if outcome.is_some() {
                break;
            }
        }

        match outcome {
            Some(Parsed::Answer(ips, ttl_ms)) => {
                cache_put(&name, ips.clone(), ttl_ms);
                return ips;
            }
            Some(Parsed::Truncated) => {
                // UDP-ответ обрезан (TC): тот же запрос по TCP/53.
                match query_tcp(*server, &query, id) {
                    Some(Parsed::Answer(ips, ttl_ms)) => {
                        cache_put(&name, ips.clone(), ttl_ms);
                        return ips;
                    }
                    Some(Parsed::Reject(why)) => {
                        push_report(&mut report, *server, &why);
                    }
                    _ => push_report(&mut report, *server, "ответ обрезан (TC), TCP/53 не ответил"),
                }
            }
            Some(Parsed::Reject(why)) => {
                // NXDOMAIN от настроенного сервера — окончательный ответ:
                // имя действительно не существует, повторять на публичных
                // серверах смысла нет. Прочие отказы (SERVFAIL, REFUSED,
                // «без A-записей») пробуем переспросить у следующих.
                if si == 0 && why.contains("NXDOMAIN") {
                    set_error(format!("'{}': {}", name, why));
                    return Vec::new();
                }
                push_report(&mut report, *server, &why);
            }
            None => {
                // Сервер молчит по UDP: последняя надежда — TCP/53 (NAT
                // иногда режет только UDP), дальше — следующий сервер.
                match query_tcp(*server, &query, id) {
                    Some(Parsed::Answer(ips, ttl_ms)) => {
                        cache_put(&name, ips.clone(), ttl_ms);
                        return ips;
                    }
                    Some(Parsed::Reject(why)) => push_report(&mut report, *server, &why),
                    _ => push_report(
                        &mut report,
                        *server,
                        &format!(
                            "нет ответа по UDP ({}×{} мс) и TCP",
                            attempts, ATTEMPT_TIMEOUT_MS
                        ),
                    ),
                }
            }
        }
    }
    if let Some(ips) = cache_stale(&name) {
        crate::diag::notice(
            crate::diag::NONE,
            &format!("DNS: нет ответа для '{}', использован прошлый ответ с истёкшим TTL", name),
        );
        return ips;
    }
    if report.is_empty() {
        report = String::from("нет DNS-серверов для запроса");
    }
    set_error(report);
    Vec::new()
}

/// Очередь серверов: настроенный (NetConfig) первым, затем публичные
/// резервные — без дублей и без 0.0.0.0.
fn server_list() -> Vec<[u8; 4]> {
    let mut out = Vec::new();
    let primary = crate::net::dns_ip();
    if primary != [0, 0, 0, 0] {
        out.push(primary);
    }
    for fb in FALLBACK_SERVERS.iter() {
        if !out.contains(fb) {
            out.push(*fb);
        }
    }
    out
}

fn push_report(report: &mut String, server: [u8; 4], why: &str) {
    if !report.is_empty() {
        report.push_str("; ");
    }
    report.push_str(&format!(
        "{}.{}.{}.{}: {}",
        server[0], server[1], server[2], server[3], why
    ));
}

/// Запрос по TCP/53 (RFC 1035 §4.2.2): длина u16 BE + тот же UDP-запрос.
fn query_tcp(server: [u8; 4], query: &[u8], id: u16) -> Option<Parsed> {
    use crate::net::tcp;
    if tcp::connect_any(&[server], DNS_PORT).is_err() {
        return None;
    }
    let mut msg = Vec::with_capacity(query.len() + 2);
    msg.extend_from_slice(&(query.len() as u16).to_be_bytes());
    msg.extend_from_slice(query);
    if !tcp::send(&msg) {
        tcp::close();
        return None;
    }
    let deadline = crate::timer::uptime_ms() + TCP_TIMEOUT_MS;
    let mut acc: Vec<u8> = Vec::new();
    loop {
        let left = deadline.saturating_sub(crate::timer::uptime_ms());
        if left == 0 {
            break;
        }
        let chunk = tcp::recv(left);
        if chunk.is_empty() {
            break; // FIN/RST или таймаут — больше данных не будет
        }
        acc.extend_from_slice(&chunk);
        if acc.len() >= 2 {
            let n = u16::from_be_bytes([acc[0], acc[1]]) as usize;
            if acc.len() >= 2 + n {
                tcp::close();
                return parse_response(&acc[2..2 + n], id);
            }
        }
    }
    tcp::close();
    None
}

/// Сбрасывает DNS-кеш (используется при смене сетевой конфигурации).
pub fn flush_cache() {
    without_interrupts(|| CACHE.lock().clear());
}
