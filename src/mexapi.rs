//! MEX API v2.0 — implementations.
#![allow(dead_code)]
use crate::fs;

pub fn api_tcp_connect(ip_ptr: *const u8, ip_len: usize, port: u16) -> i64 {
    if ip_ptr.is_null() || ip_len != 4 { return -1; }
    let s = unsafe { core::slice::from_raw_parts(ip_ptr, 4) };
    if crate::net::tcp::connect([s[0],s[1],s[2],s[3]], port) { 0 } else { -1 }
}
pub fn api_tcp_send(d: *const u8, l: usize) -> i64 {
    if d.is_null()||l==0 { -1 } else { let s=unsafe{core::slice::from_raw_parts(d,l)}; if crate::net::tcp::send(s){l as i64}else{-1} }
}
pub fn api_tcp_recv(o:*mut u8,c:usize,t:u64)->i64 {
    if o.is_null()||c==0 { -1 } else { let d=crate::net::tcp::recv(t); if d.is_empty(){-1}else{let n=d.len().min(c);unsafe{core::ptr::copy_nonoverlapping(d.as_ptr(),o,n)};n as i64} }
}
pub fn api_http_get(u:*const u8,ul:usize,o:*mut u8,c:usize)->i64 {
    if u.is_null()||o.is_null()||c<2 { -1 } else { let s=unsafe{core::slice::from_raw_parts(u,ul)}; match crate::net::http::get(core::str::from_utf8(s).unwrap_or("")){Ok(r)=>{let n=r.body.len().min(c-1);unsafe{core::ptr::copy_nonoverlapping(r.body.as_ptr(),o,n);o.add(n).write(0)};n as i64}Err(_)=>-1} }
}
pub fn api_sleep_ms(ms: u64) { let t=crate::timer::uptime_ms()+ms; while crate::timer::uptime_ms()<t{unsafe{core::arch::asm!("hlt");}} }
pub fn api_random() -> u64 { let mut b=[0u8;8]; crate::rng::fill_random(&mut b); u64::from_le_bytes(b) }
pub fn api_get_username(o:*mut u8,c:usize)->i64 { if o.is_null()||c==0{-1}else{let u=fs::current_user();let b=u.as_bytes();let n=b.len().min(c-1);unsafe{core::ptr::copy_nonoverlapping(b.as_ptr(),o,n);o.add(n).write(0)};n as i64} }
pub fn api_get_hostname(o:*mut u8,c:usize)->i64 { if o.is_null()||c==0{-1}else{let n=6.min(c-1);unsafe{o.copy_from_nonoverlapping(b"deixos".as_ptr(),n);o.add(n).write(0)};n as i64} }
