//! Минимальный TCP (RFC 793) для HTTP/браузера.
//! Использует send_packet_resolving для отправки и rx_poll для приёма.

use crate::net::ipv4;
use crate::rtl8139;
use crate::spinlock::SpinLock;
use alloc::vec::Vec;
use alloc::collections::VecDeque;

const TCP_PROTO: u8 = 6;
const FIN: u8 = 0x01; const SYN: u8 = 0x02;
const PSH: u8 = 0x08; const ACK: u8 = 0x10;

#[repr(C, packed)] #[derive(Clone, Copy)]
struct TcpHdr { src: u16, dst: u16, seq: u32, ack: u32, off_flags: u16, win: u16, csum: u16, urg: u16 }

impl TcpHdr {
    fn doff(&self) -> usize { ((u16::from_be(self.off_flags) >> 12) as usize & 0xF) * 4 }
    fn flags(&self) -> u8 { (u16::from_be(self.off_flags) & 0xFF) as u8 }
    const SZ: usize = 20;
}

pub struct TcpConn { pub state: u8, sport: u16, dip: [u8;4], dport: u16, seq: u32, ack: u32 }
static CONN: SpinLock<Option<TcpConn>> = SpinLock::new(None);
static NPORT: SpinLock<u16> = SpinLock::new(1024);

// Очередь принятых TCP-пакетов (наполняется из обработчика прерываний).
static RX_QUEUE: SpinLock<VecDeque<Vec<u8>>> = SpinLock::new(VecDeque::new());

pub fn rx_enqueue(pkt: Vec<u8>) {
    RX_QUEUE.lock().push_back(pkt);
}

fn rx_dequeue() -> Option<Vec<u8>> {
    RX_QUEUE.lock().pop_front()
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
    let hsz = TcpHdr::SZ; let mut seg = Vec::with_capacity(hsz+data.len());
    let h = TcpHdr {
        src: c.sport.to_be(), dst: c.dport.to_be(),
        seq: c.seq.to_be(), ack: c.ack.to_be(),
        off_flags: (((hsz/4) as u16)<<12|flags as u16).to_be(),
        win: 8192u16.to_be(), csum: 0, urg: 0,
    };
    let hb = unsafe { core::slice::from_raw_parts(&h as *const _ as *const u8, hsz) };
    seg.extend_from_slice(hb); seg.extend_from_slice(data);
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
    if pkt.len() > start { Vec::from(&pkt[start..]) } else { Vec::new() }
}

pub fn connect(dip: [u8;4], dport: u16) -> bool {
    if !rtl8139::is_ready() { return false; }
    let sp = { let mut p = NPORT.lock(); let v = *p; *p = if *p>=65535 {1024} else {*p+1}; v };
    let isn: u32 = 0xDEADBEEF;
    let mut c = TcpConn { state:0, sport:sp, dip, dport, seq:isn, ack:0 };
    send_seg(&c, SYN, &[]);
    let t0 = crate::timer::uptime_ms();
    loop {
        while let Some(pkt) = rx_dequeue() {
            if let Some((f, _a, s)) = parse_tcp(&pkt, sp, dport) {
                if f&SYN!=0 && f&ACK!=0 {
                    c.ack = s.wrapping_add(1); c.seq = isn.wrapping_add(1);
                    send_seg(&c, ACK, &[]);
                    c.state = 1; *CONN.lock() = Some(c); return true;
                }
            }
        }
        if crate::timer::uptime_ms()-t0 > 5000 { return false; }
        unsafe { core::arch::asm!("hlt"); }
    }
}

pub fn send(data: &[u8]) -> bool {
    let cl = CONN.lock();
    if let Some(ref c) = *cl { if c.state != 1 { return false; } send_seg(c, PSH|ACK, data); drop(cl);
        let mut l = CONN.lock(); if let Some(ref mut c) = *l {
            c.seq = c.seq.wrapping_add(data.len() as u32); } true } else { false }
}

pub fn recv(to: u64) -> Vec<u8> {
    let t0 = crate::timer::uptime_ms();
    loop {
        while let Some(pkt) = rx_dequeue() {
            let cl = CONN.lock();
            if let Some(ref c) = *cl {
                if let Some((f, _, _)) = parse_tcp(&pkt, c.sport, c.dport) {
                    if f&(PSH|ACK) != 0 { let p = tcp_payload(&pkt); if !p.is_empty() { return p; } }
                    if f&FIN != 0 { return Vec::new(); }
                }
            }
        }
        if crate::timer::uptime_ms()-t0 > to { return Vec::new(); }
        unsafe { core::arch::asm!("hlt"); }
    }
}

pub fn close() {
    let mut cl = CONN.lock();
    if let Some(ref c) = *cl { send_seg(c, FIN|ACK, &[]); }
    *cl = None;
}
