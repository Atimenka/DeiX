//! IPC ring buffer for Nano-Core.
//!
//! Shared memory + ring buffer protocol:
//! Header: {src, dst, opcode, len, checksum}
//! Max message: 64 KiB, timeout: 100 ms.
//!
//! In this no_std kernel, the "shared memory" is just a static buffer
//! in `.bss`. Processes communicate via the orchestrator's IPC hub.

use core::sync::atomic::{AtomicU32, Ordering};

use alloc::vec::Vec;

/// Max IPC message size (64 KiB).
pub const MAX_MSG_SIZE: usize = 65536;
/// IPC timeout in milliseconds.
pub const IPC_TIMEOUT_MS: u64 = 100;
/// Max ring buffer slots.
pub const RING_CAPACITY: usize = 256;

/// IPC message header.
#[derive(Debug, Clone, Copy)]
pub struct IpcHeader {
    pub src: u32,
    pub dst: u32,
    pub opcode: u32,
    pub len: u16,
    pub checksum: u16,
}

impl IpcHeader {
    pub fn new(src: u32, dst: u32, opcode: u32, len: u16) -> Self {
        let checksum = (src as u16) ^ (dst as u16) ^ (opcode as u16) ^ len;
        IpcHeader { src, dst, opcode, len, checksum }
    }

    pub fn is_valid(&self) -> bool {
        let checksum = (self.src as u16) ^ (self.dst as u16) ^ (self.opcode as u16) ^ self.len;
        checksum == self.checksum
    }
}

/// IPC message (header + payload).
#[derive(Debug, Clone)]
pub struct IpcMessage {
    pub header: IpcHeader,
    pub payload: Vec<u8>,
}

impl IpcMessage {
    pub fn new(src: u32, dst: u32, opcode: u32, payload: Vec<u8>) -> Self {
        let len = (payload.len() as u16).min(MAX_MSG_SIZE as u16);
        IpcMessage {
            header: IpcHeader::new(src, dst, opcode, len),
            payload: payload.into_iter().take(len as usize).collect(),
        }
    }
}

/// Common IPC opcodes.
pub mod opcode {
    pub const HEARTBEAT: u32 = 0x01;
    pub const LOG_PUSH: u32 = 0x02;
    pub const STATUS_QUERY: u32 = 0x03;
    pub const STATUS_RESPONSE: u32 = 0x04;
    pub const DEPENDENCY_READY: u32 = 0x05;
    pub const DEPENDENCY_FAIL: u32 = 0x06;
    pub const RESTART_REQUEST: u32 = 0x07;
    pub const SHUTDOWN: u32 = 0x08;
    pub const DISCOVER: u32 = 0x09;
    pub const RESPONSE: u32 = 0x10;
}

/// Simple IPC ring buffer (single-producer, single-consumer).
///
/// In this kernel, the orchestrator is the only consumer and Nano-Threads
/// are the producers (they push messages into the ring).
pub struct IpcRing {
    buf: Vec<Vec<u8>>,
    head: AtomicU32,
    tail: AtomicU32,
    count: AtomicU32,
}

impl IpcRing {
    pub const fn new() -> Self {
        // Vec::new() is const-constructible in const context since Rust 1.60
        // but we need a runtime init for the buffer allocation.
        // We'll use a separate init function.
        IpcRing {
            buf: Vec::new(),
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
            count: AtomicU32::new(0),
        }
    }

    pub fn init(&mut self) {
        self.buf = alloc::vec![Vec::new(); RING_CAPACITY];
    }

    pub fn push(&mut self, msg: IpcMessage) -> Result<(), String> {
        let count = self.count.load(Ordering::Relaxed);
        if count >= RING_CAPACITY as u32 {
            return Err("IPC ring full".to_string());
        }
        let slot = (self.tail.load(Ordering::Relaxed) % RING_CAPACITY as u32) as usize;
        let mut bytes: Vec<u8> = Vec::with_capacity(msg.payload.len() + 16);
        // Serialize header
        bytes.extend_from_slice(&msg.header.src.to_le_bytes());
        bytes.extend_from_slice(&msg.header.dst.to_le_bytes());
        bytes.extend_from_slice(&msg.header.opcode.to_le_bytes());
        bytes.extend_from_slice(&msg.header.len.to_le_bytes());
        bytes.extend_from_slice(&msg.header.checksum.to_le_bytes());
        bytes.extend_from_slice(&msg.payload);
        self.buf[slot] = bytes;
        self.tail.fetch_add(1, Ordering::Release);
        self.count.fetch_add(1, Ordering::Release);
        Ok(())
    }

    pub fn pop(&mut self) -> Option<IpcMessage> {
        if self.count.load(Ordering::Relaxed) == 0 {
            return None;
        }
        let slot = (self.head.load(Ordering::Relaxed) % RING_CAPACITY as u32) as usize;
        let bytes = match self.buf.get(slot) {
            Some(b) if !b.is_empty() => b.clone(),
            _ => {
                self.head.fetch_add(1, Ordering::Release);
                self.count.fetch_sub(1, Ordering::Release);
                return None;
            }
        };
        self.head.fetch_add(1, Ordering::Release);
        self.count.fetch_sub(1, Ordering::Release);

        // Deserialize header
        if bytes.len() < 16 {
            return None;
        }
        let src = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let dst = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let opcode = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        let len = u16::from_le_bytes([bytes[12], bytes[13]]);
        let checksum = u16::from_le_bytes([bytes[14], bytes[15]]);

        let header = IpcHeader { src, dst, opcode, len, checksum };
        if !header.is_valid() {
            return None;
        }
        let payload = bytes[16..16 + len as usize].to_vec();
        Some(IpcMessage { header, payload })
    }

    pub fn len(&self) -> u32 {
        self.count.load(Ordering::Relaxed)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl core::default::Default for IpcRing {
    fn default() -> Self {
        Self::new()
    }
}
