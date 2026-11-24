//! EHCI (Enhanced Host Controller Interface 1.0) data layouts: the
//! capability and operational registers, port status bits, the legacy
//! support capability in PCI configuration space, and the queue heads and
//! queue element transfer descriptors the controller reads from memory.
//!
//! EHCI drives the USB 2.0 ports of PCs from before xHCI took over: every
//! Intel chipset up to the 6 series (Sandy Bridge: H61, P67, Z68, ...) has
//! EHCI and no xHCI. Since the 5 series Intel puts a "rate matching hub"
//! behind each EHCI root port instead of companion UHCI controllers, so
//! keyboards and mice (low and full speed) are always reached through a
//! high-speed hub's transaction translator, with split transactions.
//!
//! Everything here is plain data manipulation, tested on the host; the
//! kernel driver (`kernel/src/drivers/ehci.rs`) supplies MMIO, DMA memory
//! and timing.

use super::Speed;

// ---- capability registers ----------------------------------------------------------------

pub const CAPLENGTH: u64 = 0x00;
pub const HCIVERSION: u64 = 0x02;
pub const HCSPARAMS: u64 = 0x04;
pub const HCCPARAMS: u64 = 0x08;

/// What the capability registers say about a controller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    /// Offset of the operational registers from the start of the BAR.
    pub op_offset: u64,
    pub version: u16,
    pub ports: u8,
    /// Software controls port power (PPC).
    pub port_power_control: bool,
    /// Companion (UHCI/OHCI) controllers for low and full-speed devices.
    pub companions: u8,
    /// 64-bit data structures (the extended qTD/QH layouts).
    pub ac64: bool,
    /// Offset in PCI configuration space of the first extended capability
    /// (0: none).
    pub eecp: u8,
}

impl Capabilities {
    /// From the first dword (CAPLENGTH, HCIVERSION) and the two parameter
    /// registers. `None` for values no EHCI controller has (a BAR that
    /// reads all ones, a capability length shorter than the registers).
    pub fn parse(dword0: u32, hcsparams: u32, hccparams: u32) -> Option<Capabilities> {
        let caplength = dword0 & 0xFF;
        if dword0 == u32::MAX || caplength < 0x0C {
            return None;
        }
        let ports = (hcsparams & 0xF) as u8;
        if ports == 0 {
            return None;
        }
        Some(Capabilities {
            op_offset: caplength as u64,
            version: (dword0 >> 16) as u16,
            ports,
            port_power_control: hcsparams & (1 << 4) != 0,
            companions: ((hcsparams >> 12) & 0xF) as u8,
            ac64: hccparams & 1 != 0,
            eecp: (hccparams >> 8) as u8,
        })
    }
}

// ---- operational registers ---------------------------------------------------------------

pub mod op {
    pub const USBCMD: u64 = 0x00;
    pub const USBSTS: u64 = 0x04;
    pub const USBINTR: u64 = 0x08;
    pub const FRINDEX: u64 = 0x0C;
    pub const CTRLDSSEGMENT: u64 = 0x10;
    pub const PERIODICLISTBASE: u64 = 0x14;
    pub const ASYNCLISTADDR: u64 = 0x18;
    pub const CONFIGFLAG: u64 = 0x40;

    /// PORTSC of port `n` (1-based).
    pub fn portsc(n: u8) -> u64 {
        0x44 + 4 * (n as u64 - 1)
    }
}

pub mod usbcmd {
    pub const RUN: u32 = 1 << 0;
    pub const HCRESET: u32 = 1 << 1;
    pub const PERIODIC_ENABLE: u32 = 1 << 4;
    pub const ASYNC_ENABLE: u32 = 1 << 5;
    /// Interrupt on Async Advance Doorbell.
    pub const IAAD: u32 = 1 << 6;
    /// Interrupt threshold of 8 micro-frames (1 ms), the default.
    pub const ITC_1MS: u32 = 0x08 << 16;
}

pub mod usbsts {
    pub const USBINT: u32 = 1 << 0;
    pub const USBERRINT: u32 = 1 << 1;
    pub const PORT_CHANGE: u32 = 1 << 2;
    pub const FRAME_ROLLOVER: u32 = 1 << 3;
    pub const HOST_SYSTEM_ERROR: u32 = 1 << 4;
    /// Interrupt on Async Advance: the controller let go of unlinked queue heads.
    pub const IAA: u32 = 1 << 5;
    pub const HALTED: u32 = 1 << 12;
    pub const PERIODIC_RUNNING: u32 = 1 << 14;
    pub const ASYNC_RUNNING: u32 = 1 << 15;
    /// The write-1-to-clear bits.
    pub const ACK: u32 = 0x3F;
}

pub mod portsc {
    pub const CONNECTED: u32 = 1 << 0;
    pub const CONNECT_CHANGE: u32 = 1 << 1;
    pub const ENABLED: u32 = 1 << 2;
    pub const ENABLE_CHANGE: u32 = 1 << 3;
    pub const OVER_CURRENT_CHANGE: u32 = 1 << 5;
    pub const RESET: u32 = 1 << 8;
    pub const LINE_STATUS: u32 = 3 << 10;
    /// Line status K: a low-speed device (before reset).
    pub const LINE_K: u32 = 1 << 10;
    pub const POWER: u32 = 1 << 12;
    /// The port belongs to a companion controller.
    pub const OWNER: u32 = 1 << 13;
    /// The write-1-to-clear change bits.
    pub const CHANGES: u32 = CONNECT_CHANGE | ENABLE_CHANGE | OVER_CURRENT_CHANGE;

    /// The value to write to set `set` and clear `clear` without touching
    /// anything else: the change bits are written as 0 (writing 1 would
    /// acknowledge them) and Port Enabled keeps its value (writing 0
    /// disables the port).
    pub fn update(current: u32, set: u32, clear: u32) -> u32 {
        (current & !CHANGES & !clear) | set
    }

    /// The value that acknowledges the change bits that are set.
    pub fn acknowledge(current: u32) -> u32 {
        (current & !CHANGES) | (current & CHANGES)
    }
}

// ---- legacy support (PCI configuration space) ------------------------------------------------

/// Capability id of USB Legacy Support (USBLEGSUP).
pub const CAP_LEGACY_SUPPORT: u8 = 1;
/// USBLEGSUP: the firmware owns the controller.
pub const LEGSUP_BIOS_OWNED: u32 = 1 << 16;
/// USBLEGSUP: the operating system asks for (and then owns) the controller.
pub const LEGSUP_OS_OWNED: u32 = 1 << 24;
/// USBLEGCTLSTS (at USBLEGSUP + 4): SMI enables in bits 0-15, SMI status
/// in 16-31 of which 29-31 are write-1-to-clear. Written after the handoff
/// so that no SMI is raised for USB events any more.
pub const LEGCTLSTS_QUIET: u32 = 0xE000_0000;

/// Offset of USBLEGSUP in the extended capability list that starts at
/// `eecp`; `read8` reads PCI configuration space.
pub fn find_legacy_support(eecp: u8, read8: impl Fn(u8) -> u8) -> Option<u8> {
    let mut at = eecp;
    // The list lives in configuration space above the standard header;
    // a broken one is followed for at most 48 entries.
    for _ in 0..48 {
        if at < 0x40 {
            return None;
        }
        if read8(at) == CAP_LEGACY_SUPPORT {
            return Some(at);
        }
        at = read8(at.wrapping_add(1));
    }
    None
}

// ---- schedule data structures ------------------------------------------------------------

/// Link pointer: terminate.
pub const LINK_TERMINATE: u32 = 1;
/// Link pointer type: queue head.
pub const LINK_QH: u32 = 1 << 1;

/// A queue head's link value.
pub fn qh_link(phys: u32) -> u32 {
    (phys & !0x1F) | LINK_QH
}

/// Packet identifiers of a qTD.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pid {
    Out = 0,
    In = 1,
    Setup = 2,
}

pub mod token {
    pub const ACTIVE: u32 = 1 << 7;
    pub const HALTED: u32 = 1 << 6;
    pub const BUFFER_ERROR: u32 = 1 << 5;
    pub const BABBLE: u32 = 1 << 4;
    pub const TRANSACTION_ERROR: u32 = 1 << 3;
    pub const MISSED_MICROFRAME: u32 = 1 << 2;
    /// Three retries before a transaction error halts the queue.
    pub const CERR_3: u32 = 3 << 10;
    pub const IOC: u32 = 1 << 15;
    pub const TOGGLE: u32 = 1 << 31;

    /// Bytes still to transfer when the qTD retired.
    pub fn remaining(token: u32) -> u32 {
        (token >> 16) & 0x7FFF
    }

    /// Whether the qTD ended with an error that halted its queue.
    pub fn halted(token: u32) -> bool {
        token & HALTED != 0
    }
}

/// The token of a fresh qTD, active.
pub fn qtd_token(pid: Pid, bytes: u32, toggle: bool, ioc: bool) -> u32 {
    token::ACTIVE
        | token::CERR_3
        | (pid as u32) << 8
        | (bytes & 0x7FFF) << 16
        | if ioc { token::IOC } else { 0 }
        | if toggle { token::TOGGLE } else { 0 }
}

/// A queue element transfer descriptor (the first 8 dwords; the 64-bit
/// layout's extended buffer pointers stay zero for memory below 4 GiB).
/// `buffer` is the physical address of the data; it may not cross more
/// than four page boundaries.
pub fn qtd(next: u32, alt_next: u32, token: u32, buffer: u32, bytes: u32) -> [u32; 8] {
    let mut d = [next, alt_next, token, buffer, 0, 0, 0, 0];
    let first_page = buffer & !0xFFF;
    let end = buffer as u64 + bytes as u64;
    for (i, slot) in d[4..8].iter_mut().enumerate() {
        let page = first_page as u64 + 0x1000 * (i as u64 + 1);
        if page < end {
            *slot = page as u32;
        }
    }
    d
}

/// The EHCI speed code (EPS) of a queue head.
pub fn eps(speed: Speed) -> u32 {
    match speed {
        Speed::Full => 0,
        Speed::Low => 1,
        _ => 2,
    }
}

/// What a queue head describes: one endpoint of one device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub address: u8,
    pub number: u8,
    pub speed: Speed,
    pub max_packet: u16,
    /// The control endpoint (data toggle from the qTDs; for a low or
    /// full-speed device the Control Endpoint flag).
    pub control: bool,
    /// The high-speed hub and port whose transaction translator reaches a
    /// low or full-speed device; `None` for a high-speed device.
    pub tt: Option<(u8, u8)>,
}

/// Micro-frames (bits of a frame) an interrupt endpoint is served in:
/// (S-mask, C-mask). A high-speed endpoint is polled in micro-frame 0; a
/// low or full-speed one behind a transaction translator gets its start
/// split in micro-frame 0 and complete splits in 2, 3 and 4, the budget
/// EHCI 4.12.2.4 gives for a transaction that starts at the beginning of
/// a frame.
pub fn interrupt_masks(speed: Speed) -> (u8, u8) {
    match speed {
        Speed::Low | Speed::Full => (0x01, 0x1C),
        _ => (0x01, 0x00),
    }
}

/// The first 12 dwords of a queue head (horizontal link, endpoint
/// characteristics and capabilities, current qTD, and the overlay with
/// `first` as its next qTD). `head`: the head of the reclamation list.
/// `periodic`: an interrupt endpoint, with the masks of
/// [`interrupt_masks`].
pub fn queue_head(link: u32, ep: &Endpoint, first: u32, head: bool, periodic: bool) -> [u32; 12] {
    let high = !matches!(ep.speed, Speed::Low | Speed::Full);
    let characteristics = (ep.address as u32 & 0x7F)
        | (ep.number as u32 & 0xF) << 8
        | eps(ep.speed) << 12
        | if ep.control { 1 << 14 } else { 0 }
        | if head { 1 << 15 } else { 0 }
        | (ep.max_packet as u32 & 0x7FF) << 16
        | if ep.control && !high { 1 << 27 } else { 0 }
        // NAK count reload: only for high-speed asynchronous endpoints.
        | if high && !periodic { 4 << 28 } else { 0 };
    let (s, c) = if periodic { interrupt_masks(ep.speed) } else { (0, 0) };
    let (hub, port) = ep.tt.filter(|_| !high).unwrap_or((0, 0));
    let capabilities = s as u32 | (c as u32) << 8 | (hub as u32 & 0x7F) << 16 | (port as u32 & 0x7F) << 23 | 1 << 30;
    [link, characteristics, capabilities, 0, first, LINK_TERMINATE, 0, 0, 0, 0, 0, 0]
}

/// The overlay token that keeps a queue head out of service: halted.
pub const QH_PARKED: u32 = token::HALTED;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities() {
        // Intel 6 series EHCI: CAPLENGTH 0x20, version 1.00, 2 ports with
        // port power control (the rate matching hubs sit behind them), no
        // companions, 64-bit, EECP 0xA0.
        let c = Capabilities::parse(0x0100_0020, 0x0020_3012 & !0xF000 | 0x2, 0x0000_A0E1 | 1).unwrap();
        assert_eq!((c.op_offset, c.version, c.ports, c.port_power_control, c.companions, c.ac64, c.eecp), (0x20, 0x0100, 2, true, 0, true, 0xA0));
        // QEMU's ICH4 EHCI: 6 ports, three companions, 32-bit, no EECP.
        let c = Capabilities::parse(0x0100_0020, 0x0000_3306 | 0x10, 0x0000_0000).unwrap();
        assert_eq!((c.ports, c.companions, c.ac64, c.eecp), (6, 3, false, 0));
        assert_eq!(Capabilities::parse(u32::MAX, 0, 0), None);
        assert_eq!(Capabilities::parse(0x0100_0004, 2, 0), None);
        assert_eq!(Capabilities::parse(0x0100_0020, 0, 0), None);
        assert_eq!(op::portsc(1), 0x44);
        assert_eq!(op::portsc(3), 0x4C);
    }

    #[test]
    fn port_writes_keep_enable_and_changes() {
        let v = portsc::CONNECTED | portsc::CONNECT_CHANGE | portsc::ENABLED | portsc::POWER | portsc::ENABLE_CHANGE;
        // Starting a reset: the change bits are not acknowledged by accident,
        // the enable bit is cleared because the reset asks for it.
        let w = portsc::update(v, portsc::RESET, portsc::ENABLED);
        assert_eq!(w, portsc::CONNECTED | portsc::POWER | portsc::RESET);
        // Ending it keeps everything else.
        assert_eq!(portsc::update(w, 0, portsc::RESET), portsc::CONNECTED | portsc::POWER);
        // Acknowledging writes exactly the change bits that are set.
        assert_eq!(portsc::acknowledge(v) & portsc::CHANGES, portsc::CONNECT_CHANGE | portsc::ENABLE_CHANGE);
        assert_eq!(portsc::acknowledge(portsc::CONNECTED) & portsc::CHANGES, 0);
    }

    #[test]
    fn legacy_support_is_found_in_the_capability_list() {
        // 0xA0: id 0x0A (debug port), next 0x68; 0x68: legacy support.
        let mut cfg = [0u8; 256];
        cfg[0xA0] = 0x0A;
        cfg[0xA1] = 0x68;
        cfg[0x68] = CAP_LEGACY_SUPPORT;
        assert_eq!(find_legacy_support(0xA0, |o| cfg[o as usize]), Some(0x68));
        assert_eq!(find_legacy_support(0x68, |o| cfg[o as usize]), Some(0x68));
        assert_eq!(find_legacy_support(0, |o| cfg[o as usize]), None);
        // A list that loops is given up.
        let mut looped = [0u8; 256];
        looped[0x80] = 0x0A;
        looped[0x81] = 0x80;
        assert_eq!(find_legacy_support(0x80, |o| looped[o as usize]), None);
    }

    #[test]
    fn qtds() {
        let t = qtd_token(Pid::Setup, 8, false, false);
        assert_eq!(t, 0x0008_0E80);
        let t = qtd_token(Pid::In, 64, true, true);
        assert_eq!(t, 0x8040_8D80);
        assert_eq!(token::remaining(t), 64);
        assert!(!token::halted(t));
        assert!(token::halted(token::HALTED | 0x0004_0000));
        // A buffer inside one page needs one pointer; one that crosses a
        // page boundary gets the next page in the second.
        let d = qtd(0x1000, LINK_TERMINATE, t, 0x0020_0800, 0x800);
        assert_eq!(d, [0x1000, 1, t, 0x0020_0800, 0, 0, 0, 0]);
        let d = qtd(1, 1, t, 0x0020_0F00, 0x200);
        assert_eq!(&d[3..6], &[0x0020_0F00, 0x0020_1000, 0]);
        let d = qtd(1, 1, t, 0x0030_0000, 0x5000);
        assert_eq!(&d[3..8], &[0x0030_0000, 0x0030_1000, 0x0030_2000, 0x0030_3000, 0x0030_4000]);
    }

    #[test]
    fn queue_heads() {
        // The default control endpoint of a high-speed device at address 0.
        let hs = Endpoint { address: 0, number: 0, speed: Speed::High, max_packet: 64, control: true, tt: None };
        let q = queue_head(qh_link(0x5000), &hs, 0x6080, false, false);
        assert_eq!(q[0], 0x5002);
        assert_eq!(q[1], 0x4040_6000, "address 0, endpoint 0, high speed, DTC, max packet 64, NAK reload 4");
        assert_eq!(q[2], 0x4000_0000, "one transaction per micro-frame, no masks, no hub");
        assert_eq!((q[3], q[4], q[5]), (0, 0x6080, 1));
        // The reclamation list head.
        assert_eq!(queue_head(qh_link(0x5000), &hs, 1, true, false)[1] & (1 << 15), 1 << 15);
        // A low-speed keyboard's control endpoint behind hub 1 port 3: the
        // control endpoint flag, split fields, no NAK reload.
        let ls = Endpoint { address: 5, number: 0, speed: Speed::Low, max_packet: 8, control: true, tt: Some((1, 3)) };
        let q = queue_head(LINK_TERMINATE, &ls, 0x7080, false, false);
        assert_eq!(q[1], 0x0808_5005);
        assert_eq!(q[2], 0x4000_0000 | 1 << 16 | 3 << 23);
        // Its interrupt endpoint: start split in micro-frame 0, complete
        // splits in 2-4, toggle kept by the queue head.
        let int = Endpoint { address: 5, number: 1, speed: Speed::Low, max_packet: 8, control: false, tt: Some((1, 3)) };
        let q = queue_head(LINK_TERMINATE, &int, 0x7080, false, true);
        assert_eq!(q[1], 0x0008_1105);
        assert_eq!(q[2], 0x4000_0000 | 1 << 16 | 3 << 23 | 0x1C01);
        // A full-speed mouse: EPS 0.
        let fs = Endpoint { speed: Speed::Full, max_packet: 64, ..int };
        assert_eq!(queue_head(LINK_TERMINATE, &fs, 0, false, true)[1], 0x0040_0105);
        // A high-speed interrupt endpoint: no split fields even if a TT is given.
        let hsi = Endpoint { speed: Speed::High, tt: Some((1, 3)), ..int };
        let q = queue_head(LINK_TERMINATE, &hsi, 0, false, true);
        assert_eq!(q[2], 0x4000_0001);
        assert_eq!(q[1] >> 28, 0, "no NAK reload in the periodic schedule");
    }
}
