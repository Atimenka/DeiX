//! Минимальный TCP (RFC 793) для HTTP/браузера.
//! Использует send_packet_resolving для отправки и rx_poll для приёма.

use crate::net::ipv4;
use crate::rtl8139;
use crate::spinlock::SpinLock;
use alloc::vec::Vec;
use alloc::collections::VecDeque;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

const TCP_PROTO: u8 = 6;
const FIN: u8 = 0x01; const SYN: u8 = 0x02;
const RST: u8 = 0x04; const PSH: u8 = 0x08; const ACK: u8 = 0x10;

#[repr(C, packed)] #[derive(Clone, Copy)]
struct TcpHdr { src: u16, dst: u16, seq: u32, ack: u32, off_flags: u16, win: u16, csum: u16, urg: u16 }

impl TcpHdr {
    fn doff(&self) -> usize { ((u16::from_be(self.off_flags) >> 12) as usize & 0xF) * 4 }
    fn flags(&self) -> u8 { (u16::from_be(self.off_flags) & 0xFF) as u8 }
    const SZ: usize = 20;
}

#[derive(Clone, Copy)]
pub struct TcpConn { pub state: u8, sport: u16, dip: [u8;4], dport: u16, seq: u32, ack: u32 }
static CONN: SpinLock<Option<TcpConn>> = SpinLock::new(None);
static NPORT: SpinLock<u16> = SpinLock::new(1024);

// Диагностика приёма: почему recv вернул пустой результат и сколько
// байт данных принято с начала текущего соединения.
static LAST_END: SpinLock<&'static str> = SpinLock::new("данных ещё не ждали");
static RX_BYTES: AtomicU32 = AtomicU32::new(0);
static DUP_DROPPED: AtomicU32 = AtomicU32::new(0);
static GAP_DROPPED: AtomicU32 = AtomicU32::new(0);
/// Наш seq сразу после последней отправки и наибольший ack от сервера.
static SENT_END: AtomicU32 = AtomicU32::new(0);
static PEER_ACK: AtomicU32 = AtomicU32::new(0);
/// Отправленные сегменты, которые сервер ещё не подтвердил: (seq, данные).
static UNACKED: SpinLock<Vec<(u32, Vec<u8>)>> = SpinLock::new(Vec::new());
static LAST_TX_MS: AtomicU64 = AtomicU64::new(0);
static RTO_MS: AtomicU32 = AtomicU32::new(RTO_INITIAL_MS);
static RETRANSMITS: AtomicU32 = AtomicU32::new(0);
static CUR_RETRIES: AtomicU32 = AtomicU32::new(0);
const RTO_INITIAL_MS: u32 = 1000;
const RTO_MAX_MS: u32 = 8000;
const MAX_RETRIES: u32 = 5;
/// Паузы перед повторной отправкой SYN: 1 с, 2 с, 4 с.
const SYN_WAITS_MS: [u64; 3] = [1000, 2000, 4000];

/// Начальный seq (ISN) всех наших соединений.
const ISN: u32 = 0xDEADBEEF;

/// Причина последнего пустого результата `recv`.
pub fn last_end() -> &'static str {
    *LAST_END.lock()
}

/// Сводка по соединению: сколько байт отправлено и сколько из них
/// подтвердил сервер, сколько сегментов отброшено как дубликаты и
/// сколько как пришедшие с пропуском.
pub fn link_stats() -> alloc::string::String {
    let base = ISN.wrapping_add(1);
    let sent = SENT_END.load(Ordering::Relaxed).wrapping_sub(base);
    let acked = PEER_ACK.load(Ordering::Relaxed).wrapping_sub(base);
    alloc::format!(
        "отправлено {} байт, сервер подтвердил {}, повторных отправок {}; сегментов-дубликатов {}, с пропуском {}",
        sent,
        acked,
        RETRANSMITS.load(Ordering::Relaxed),
        DUP_DROPPED.load(Ordering::Relaxed),
        GAP_DROPPED.load(Ordering::Relaxed)
    )
}

/// Сколько байт TCP-данных принято в текущем соединении.
pub fn rx_bytes() -> u32 {
    RX_BYTES.load(Ordering::Relaxed)
}

// Очередь принятых TCP-пакетов (наполняется из обработчика прерываний).
static RX_QUEUE: SpinLock<VecDeque<Vec<u8>>> = SpinLock::new(VecDeque::new());

pub fn rx_enqueue(pkt: Vec<u8>) {
    RX_QUEUE.lock().push_back(pkt);
}

fn rx_dequeue() -> Option<Vec<u8>> {
    RX_QUEUE.lock().pop_front()
}

fn rx_dequeue_len() -> usize {
    RX_QUEUE.lock().len()
}

fn tcp_csum(src: &[u8;4], dst: &[u8;4], seg: &[u8]) -> u16 {
    let mut s: u32 = 0;
    s += ((src[0] as u32)<<8)|(src[1] as u32); s += ((src[2] as u32)<<8)|(src[3] as u32);
    s += ((dst[0] as u32)<<8)|(dst[1] as u32); s += ((dst[2] as u32)<<8)|(dst[3] as u32);
    s += TCP_PROTO as u32; s += seg.len() as u32;
    let mut i = 0;
    while i+1 < seg.len() { s += ((seg[i] as u32)<<8)|(seg[i+1] as u32); i+=2; }
    if i < seg.len() { s += (seg[i] as u32)<<8; }
    while s>>16 != 0 { s = (s&0xFFFF)+(s>>16); }
    !(s as u16)
}

fn send_seg(c: &TcpConn, flags: u8, data: &[u8]) {
    let mut hsz = TcpHdr::SZ;
    // MSS-опция в SYN: без неё сервер шлёт сегменты по 536 байт и передача
    // большого файла идёт в разы дольше. MSS=1460 (kind 2, len 4).
    let mut opts: [u8; 4] = [2, 4, 0x05, 0xB4];
    if flags & SYN == 0 { opts = [0; 4]; }
    if flags & SYN != 0 { hsz += 4; }
    let mut seg = Vec::with_capacity(hsz+data.len());
    let h = TcpHdr {
        src: c.sport.to_be(), dst: c.dport.to_be(),
        seq: c.seq.to_be(), ack: c.ack.to_be(),
        off_flags: (((hsz/4) as u16)<<12|flags as u16).to_be(),
        // Окно 8K < RX-буфера QEMU rtl8139 (~16K): сервер шлёт порциями,
        // которые гарантированно влезают в буфер; QEMU заливает весь ответ
        // сразу, и большее окно приводит к переполнению RX и потере пакетов.
        win: 8192u16.to_be(), csum: 0, urg: 0,
    };
    let hb = unsafe { core::slice::from_raw_parts(&h as *const _ as *const u8, TcpHdr::SZ) };
    seg.extend_from_slice(hb);
    if flags & SYN != 0 { seg.extend_from_slice(&opts); }
    seg.extend_from_slice(data);
    let cs = tcp_csum(&crate::net::my_ip(), &c.dip, &seg);
    seg[16] = (cs>>8) as u8; seg[17] = (cs&0xFF) as u8;
    ipv4::send_packet_resolving(c.dip, TCP_PROTO, &seg, 0);
}

fn parse_tcp(pkt: &[u8], op: u16, dp: u16) -> Option<(u8,u32,u32)> {
    if pkt.len() < 14+20+TcpHdr::SZ { return None; }
    let iplen = ((pkt[14]&0x0F)*4) as usize;
    let tcp = 14+iplen;
    if pkt.len() < tcp+TcpHdr::SZ { return None; }
    let h = unsafe { &*(pkt.as_ptr().add(tcp) as *const TcpHdr) };
    let sp = u16::from_be(h.src); let dp2 = u16::from_be(h.dst);
    if !((sp==dp && dp2==op) || (sp==op && dp2==dp)) { return None; }
    Some((h.flags(), u32::from_be(h.ack), u32::from_be(h.seq)))
}

fn tcp_payload(pkt: &[u8]) -> Vec<u8> {
    let iplen = ((pkt[14]&0x0F)*4) as usize;
    let tcp = 14+iplen;
    if pkt.len() < tcp+TcpHdr::SZ { return Vec::new(); }
    let h = unsafe { &*(pkt.as_ptr().add(tcp) as *const TcpHdr) };
    let start = tcp + h.doff();
    // Короткие кадры Ethernet добиваются нулями до 60 байт: данные TCP
    // заканчиваются по полю Total Length заголовка IPv4, а не по концу кадра.
    let ip_total = u16::from_be_bytes([pkt[16], pkt[17]]) as usize;
    let end = (14 + ip_total).min(pkt.len());
    if end > start { Vec::from(&pkt[start..end]) } else { Vec::new() }
}

pub fn connect(dip: [u8;4], dport: u16) -> bool {
    if !rtl8139::is_ready() { return false; }
    let sp = { let mut p = NPORT.lock(); let v = *p; *p = if *p>=65535 {1024} else {*p+1}; v };
    let isn: u32 = ISN;
    let mut c = TcpConn { state:0, sport:sp, dip, dport, seq:isn, ack:0 };
    RX_BYTES.store(0, Ordering::Relaxed);
    DUP_DROPPED.store(0, Ordering::Relaxed);
    GAP_DROPPED.store(0, Ordering::Relaxed);
    RETRANSMITS.store(0, Ordering::Relaxed);
    CUR_RETRIES.store(0, Ordering::Relaxed);
    RTO_MS.store(RTO_INITIAL_MS, Ordering::Relaxed);
    UNACKED.lock().clear();
    SENT_END.store(isn.wrapping_add(1), Ordering::Relaxed);
    PEER_ACK.store(isn.wrapping_add(1), Ordering::Relaxed);
    *LAST_END.lock() = "данных ещё не ждали";
    for wait in SYN_WAITS_MS {
        send_seg(&c, SYN, &[]);
        let t0 = crate::timer::uptime_ms();
        loop {
            crate::rtl8139::poll_rx();
            while let Some(pkt) = rx_dequeue() {
                if let Some((f, _a, s)) = parse_tcp(&pkt, sp, dport) {
                    if f&RST != 0 {
                        *LAST_END.lock() = "сервер отклонил соединение (RST на SYN)";
                        return false;
                    }
                    if f&SYN!=0 && f&ACK!=0 {
                        c.ack = s.wrapping_add(1); c.seq = isn.wrapping_add(1);
                        send_seg(&c, ACK, &[]);
                        c.state = 1; *CONN.lock() = Some(c); return true;
                    }
                }
            }
            if crate::timer::uptime_ms()-t0 > wait { break; }
            crate::sched::idle_wait();
        }
    }
    *LAST_END.lock() = "нет SYN-ACK за 7 с (3 попытки SYN)";
    false
}

/// Причина, по которой не удалось открыть соединение.
pub fn connect_failure() -> &'static str {
    last_end()
}

pub fn send(data: &[u8]) -> bool {
    let mut cl = CONN.lock();
    let c = match *cl {
        Some(ref mut c) if c.state == 1 => c,
        _ => return false,
    };
    send_seg(c, PSH|ACK, data);
    UNACKED.lock().push((c.seq, data.to_vec()));
    c.seq = c.seq.wrapping_add(data.len() as u32);
    SENT_END.store(c.seq, Ordering::Relaxed);
    LAST_TX_MS.store(crate::timer::uptime_ms(), Ordering::Relaxed);
    RTO_MS.store(RTO_INITIAL_MS, Ordering::Relaxed);
    CUR_RETRIES.store(0, Ordering::Relaxed);
    true
}

/// Повторно отправляет неподтверждённые сегменты, если сервер молчит
/// дольше текущего RTO (удваивается до RTO_MAX_MS, не более MAX_RETRIES
/// попыток на отправку). Вызывается из ожидания в `recv`.
fn retransmit_if_due() {
    let now = crate::timer::uptime_ms();
    let rto = RTO_MS.load(Ordering::Relaxed) as u64;
    if now.saturating_sub(LAST_TX_MS.load(Ordering::Relaxed)) < rto {
        return;
    }
    if CUR_RETRIES.load(Ordering::Relaxed) >= MAX_RETRIES {
        return;
    }
    let conn = match *CONN.lock() {
        Some(c) => c,
        None => return,
    };
    let pending = (*UNACKED.lock()).clone();
    if pending.is_empty() {
        return;
    }
    for (seq, data) in pending.iter() {
        let mut seg = conn;
        seg.seq = *seq;
        send_seg(&seg, PSH|ACK, data);
    }
    RETRANSMITS.fetch_add(1, Ordering::Relaxed);
    CUR_RETRIES.fetch_add(1, Ordering::Relaxed);
    LAST_TX_MS.store(now, Ordering::Relaxed);
    RTO_MS.store((rto as u32).saturating_mul(2).min(RTO_MAX_MS), Ordering::Relaxed);
}

pub fn recv(to: u64) -> Vec<u8> {
    let t0 = crate::timer::uptime_ms();
    loop {
        if rx_dequeue_len() == 0 {
            // Страховка: читаем RX-буфер RTL8139 напрямую, не полагаясь на
            // прерывания (QEMU заливает большой ответ быстрее, чем приходят
            // IRQ, и без поллинга буфер переполняется и пакеты теряются).
            crate::rtl8139::poll_rx();
        }
        while let Some(pkt) = rx_dequeue() {
            let cl = CONN.lock();
            let c = match *cl {
                Some(ref c) => c,
                None => continue,
            };
            let (f, peer_ack, s) = match parse_tcp(&pkt, c.sport, c.dport) {
                Some(v) => v,
                None => continue,
            };
            if f&RST != 0 {
                *LAST_END.lock() = "сервер сбросил соединение (RST)";
                return Vec::new();
            }
            if f&ACK != 0 && (peer_ack.wrapping_sub(PEER_ACK.load(Ordering::Relaxed)) as i32) > 0 {
                PEER_ACK.store(peer_ack, Ordering::Relaxed);
                UNACKED.lock().retain(|(seq, d)| {
                    (seq.wrapping_add(d.len() as u32).wrapping_sub(peer_ack) as i32) > 0
                });
            }
            if f&(PSH|ACK) != 0 {
                let p = tcp_payload(&pkt);
                if !p.is_empty() {
                    // Принимаем только данные, продолжающие поток: дубликаты
                    // (повторная отправка сервера) отбрасываем, перекрытие
                    // обрезаем, сегмент с пропуском не принимаем. На любое
                    // отклонение отвечаем текущим ACK, чтобы сервер повторил.
                    let gap = s.wrapping_sub(c.ack) as i32;
                    let skip = if gap < 0 { gap.unsigned_abs() as usize } else { 0 };
                    if gap > 0 || skip >= p.len() {
                        if gap > 0 {
                            GAP_DROPPED.fetch_add(1, Ordering::Relaxed);
                        } else {
                            DUP_DROPPED.fetch_add(1, Ordering::Relaxed);
                        }
                        let nc = *c;
                        send_seg(&nc, ACK, &[]);
                        continue;
                    }
                    let fresh = p[skip..].to_vec();
                    let mut nc = *c;
                    nc.ack = c.ack.wrapping_add(fresh.len() as u32);
                    drop(cl);
                    RX_BYTES.fetch_add(fresh.len() as u32, Ordering::Relaxed);
                    *CONN.lock() = Some(nc);
                    // Подтверждаем полученные данные (window update):
                    // иначе сервер упирается в окно и передача встаёт.
                    send_seg(&nc, ACK, &[]);
                    return fresh;
                }
            }
            if f&FIN != 0 {
                *LAST_END.lock() = "сервер закрыл TCP-соединение (FIN)";
                return Vec::new();
            }
        }
        retransmit_if_due();
        if crate::timer::uptime_ms()-t0 > to {
            *LAST_END.lock() = "таймаут: от сервера нет данных";
            return Vec::new();
        }
        crate::sched::idle_wait();
    }
}

pub fn close() {
    let mut cl = CONN.lock();
    if let Some(ref c) = *cl { send_seg(c, FIN|ACK, &[]); }
    *cl = None;
}
