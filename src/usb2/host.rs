//! Adapted from OSyx EHCI driver, GPL-3.0, commit 1b1665e9add219c9ea1f38fa9bbbf7290d112978.
//! EHCI host controllers: the USB 2.0 ports of PCs without xHCI, and the
//! USB 2.0 ports beside it on many that have one.
//!
//! Every Intel chipset up to the 6 series (Sandy Bridge: H61, P67, Z68,
//! ...) drives its USB ports with two EHCI controllers and nothing else.
//! Since the 5 series each EHCI root port carries a "rate matching hub":
//! keyboards and mice (low and full speed) sit behind a high-speed hub and
//! are reached with split transactions through its transaction translator.
//! Without this driver such a machine had USB input only through the
//! firmware's emulation of a PS/2 keyboard, which ends when a device is
//! plugged in again, and no mouse at all.
//!
//! The driver takes the controller from the firmware (USB legacy support
//! handoff) at boot, before the PS/2 driver looks at ports 0x60/0x64; then
//! one kernel thread per controller owns it:
//!
//! * control transfers on the asynchronous schedule: one queue head linked
//!   behind the reclamation head per transfer, unlinked with the async
//!   advance doorbell before its memory is used again;
//! * HID interrupt endpoints on the periodic schedule: a queue head per
//!   endpoint behind a parked anchor every frame points to, polled every
//!   frame (1 ms), with one qTD re-armed after each report;
//! * root ports polled every 100 ms and on a port change; hubs (the rate
//!   matching hub, external hubs) through their status change endpoint on
//!   the periodic schedule, so the control traffic that pauses the HID
//!   endpoints only happens when something is plugged or unplugged (a hub
//!   without that endpoint is asked every 100 ms);
//! * boot keyboards, mice, tablets: `drivers::usbhid`, shared with xHCI.
//!
//! The controller's interrupt is not used: the thread polls. Mass storage
//! and other classes behind EHCI are logged and left alone.
//!
//! Data layouts and their tests: `osyx_usb::ehci`.

use super::{descriptor, ehci as e, Speed};
use super::descriptor::{Configuration, DescriptorError, DeviceDescriptor, Interface, SetupPacket};
use super::ehci::{op, portsc, token, usbcmd, usbsts, Capabilities, Endpoint, Pid, LINK_TERMINATE};
use super::hid_host::{self as usbhid, ControlPipe, HidFunction};
use crate::{pci, sched};
use crate::spinlock::IrqSpinLock;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{fence, Ordering};
const PAGE_SIZE: usize = 4096;
macro_rules! kprintln { ($($arg:tt)*) => { crate::serial_println!($($arg)*) } }
struct Deadline(u64);
impl Deadline {
    fn after_ms(ms: u64) -> Self { Self(crate::timer::uptime_ms().saturating_add(ms)) }
    fn expired(&self) -> bool { crate::timer::uptime_ms() >= self.0 }
}
mod timer { pub fn sleep_ms(ms: u64) { crate::sched::sleep_ms(ms.max(10)); } }
fn phys_to_virt(addr: u64) -> u64 { addr }

/// PCI class of an EHCI controller: serial bus, USB, programming interface 0x20.
const PROG_IF_EHCI: u8 = 0x20;

/// Root port and hub port debounce before a reset (USB 2.0 7.1.7.3).
const DEBOUNCE_MS: u64 = 100;
/// Root port reset length (USB 2.0 7.1.7.5: at least 50 ms from the root).
const ROOT_RESET_MS: u64 = 50;
/// Recovery after a reset before the first transfer.
const RESET_RECOVERY_MS: u64 = 10;
const CONTROL_TIMEOUT_MS: u64 = 1_000;
/// How often root ports and hub ports are looked at without a change.
const PORT_POLL_MS: u64 = 100;
/// Attempts at bringing up a device before it is given up.
const ATTACH_ATTEMPTS: u32 = 3;
/// Consecutive failed reports before an interrupt endpoint is left alone.
const MAX_PIPE_ERRORS: u32 = 16;
/// Hubs between a root port and a device (USB 2.0 allows five).
const MAX_DEPTH: u8 = 5;

// Layout of the frames shared with the controller. 64-bit capable
// controllers use 68-byte queue heads and 52-byte qTDs; everything is
// spaced for those.
const HEAD_QH: usize = 0x00;
const ANCHOR_QH: usize = 0x80;
const CTL_QH: usize = 0x00;
const CTL_SETUP_TD: usize = 0x80;
const CTL_DATA_TD: usize = 0xC0;
const CTL_STATUS_TD: usize = 0x100;
const CTL_SETUP_BUF: usize = 0x180;
const CTL_DATA: usize = 0x800;
const CTL_MAX: usize = 0x800;
const PIPE_QH: usize = 0x00;
const PIPE_TD: usize = 0x80;
const PIPE_BUF: usize = 0x100;
const PIPE_MAX: u32 = 0xF00;
/// Dwords of a queue head: overlay next qTD and token.
const QH_NEXT: usize = 4 * 4;
const QH_TOKEN: usize = 6 * 4;
/// Dword of a qTD: token.
const TD_TOKEN: usize = 2 * 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EhciError {
    Bar(&'static str),
    OutOfMemory,
    Timeout(&'static str),
    /// The device answered STALL.
    Stall,
    /// A transfer ended with an error (the qTD token says which).
    Transfer(u32),
    Descriptor(DescriptorError),
    NoAddress,
    PortGone,
    Unsupported(&'static str),
}

impl fmt::Display for EhciError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EhciError::Bar(s) => write!(f, "BAR: {s}"),
            EhciError::OutOfMemory => write!(f, "no DMA memory below 4 GiB"),
            EhciError::Timeout(s) => write!(f, "timeout: {s}"),
            EhciError::Stall => write!(f, "the device stalled the request"),
            EhciError::Transfer(t) => write!(
                f,
                "transfer error (token {t:#010x}:{}{}{}{})",
                if t & token::BABBLE != 0 { " babble" } else { "" },
                if t & token::TRANSACTION_ERROR != 0 { " transaction error" } else { "" },
                if t & token::BUFFER_ERROR != 0 { " buffer error" } else { "" },
                if t & token::MISSED_MICROFRAME != 0 { " missed micro-frame" } else { "" }
            ),
            EhciError::Descriptor(d) => write!(f, "descriptor: {d:?}"),
            EhciError::NoAddress => write!(f, "no free device address"),
            EhciError::PortGone => write!(f, "the device went away"),
            EhciError::Unsupported(s) => write!(f, "not supported: {s}"),
        }
    }
}

impl From<DescriptorError> for EhciError {
    fn from(d: DescriptorError) -> Self {
        EhciError::Descriptor(d)
    }
}

type Result<T> = core::result::Result<T, EhciError>;

/// One zeroed frame below 4 GiB shared with the controller. It goes back
/// to the allocator when dropped: only once the controller cannot use it.
struct Frame {
    phys: u64,
    virt: u64,
}

impl Frame {
    fn new() -> Result<Frame> {
        let layout = core::alloc::Layout::from_size_align(PAGE_SIZE, PAGE_SIZE).map_err(|_| EhciError::OutOfMemory)?;
        let ptr = unsafe { alloc::alloc::alloc_zeroed(layout) };
        if ptr.is_null() { return Err(EhciError::OutOfMemory); }
        let phys = ptr as u64;
        if phys >= 0x4000_0000 || phys + PAGE_SIZE as u64 > 0x4000_0000 {
            unsafe { alloc::alloc::dealloc(ptr, layout); }
            return Err(EhciError::OutOfMemory);
        }
        Ok(Frame { phys, virt: phys })
    }

    fn p32(&self, off: usize) -> u32 {
        (self.phys + off as u64) as u32
    }

    fn r32(&self, off: usize) -> u32 {
        debug_assert!(off + 4 <= PAGE_SIZE as usize && off.is_multiple_of(4));
        // SAFETY: inside this frame, aligned; the frame is ours.
        unsafe { read_volatile((self.virt + off as u64) as *const u32) }
    }

    fn w32(&self, off: usize, v: u32) {
        debug_assert!(off + 4 <= PAGE_SIZE as usize && off.is_multiple_of(4));
        // SAFETY: as in `r32`.
        unsafe { write_volatile((self.virt + off as u64) as *mut u32, v) }
    }

    fn write(&self, off: usize, words: &[u32]) {
        for (i, w) in words.iter().enumerate() {
            self.w32(off + 4 * i, *w);
        }
    }

    fn copy_in(&self, off: usize, data: &[u8]) {
        assert!(off + data.len() <= PAGE_SIZE as usize);
        // SAFETY: inside this frame.
        unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), (self.virt + off as u64) as *mut u8, data.len()) }
    }

    fn copy_out(&self, off: usize, data: &mut [u8]) {
        assert!(off + data.len() <= PAGE_SIZE as usize);
        // SAFETY: inside this frame; the controller has finished with it.
        unsafe { core::ptr::copy_nonoverlapping((self.virt + off as u64) as *const u8, data.as_mut_ptr(), data.len()) }
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        let layout = core::alloc::Layout::from_size_align(PAGE_SIZE, PAGE_SIZE).unwrap();
        unsafe { alloc::alloc::dealloc(self.phys as *mut u8, layout); }
    }
}

/// An interrupt IN endpoint on the periodic schedule.
struct Pipe {
    frame: Frame,
    len: u32,
    kind: PipeKind,
    errors: u32,
}

enum PipeKind {
    /// A HID function's reports.
    Hid(Box<HidFunction>),
    /// A hub's status change bitmap (bit 0 the hub, bit n port n), with
    /// the changes not handled yet.
    Hub { pending: u32 },
}

struct Hub {
    ports: u8,
    /// The hub reports changes through its status change endpoint.
    notifies: bool,
    /// Hub port -> address of the device on it.
    children: BTreeMap<u8, u8>,
}

struct Device {
    speed: Speed,
    max_packet0: u16,
    tt: Option<(u8, u8)>,
    depth: u8,
    name: String,
    hub: Option<Hub>,
    pipes: Vec<Pipe>,
}

struct Ehci {
    index: usize,
    /// Virtual address of the operational registers.
    op: u64,
    caps: Capabilities,
    frame_list: Frame,
    /// The reclamation head of the asynchronous schedule and the anchor
    /// of the periodic one.
    heads: Frame,
    /// The queue head, qTDs and buffers of the one control transfer at a time.
    ctl: Frame,
    /// Physical addresses of the interrupt queue heads behind the anchor, in order.
    chain: Vec<u32>,
    devices: BTreeMap<u8, Device>,
    /// Root port -> address of the device on it.
    root: BTreeMap<u8, u8>,
    used: [bool; 128],
}

static STATUS: IrqSpinLock<Vec<String>> = IrqSpinLock::new(Vec::new());

fn set_status(index: usize, s: String) {
    let mut st = STATUS.lock();
    if st.len() <= index {
        st.resize(index + 1, String::new());
    }
    st[index] = s;
}

/// One line per controller for the kernel's status report.
pub fn status() -> Vec<String> {
    STATUS.lock().clone()
}

// PCI config mechanism #1. Byte and word writes avoid clearing status bits.
fn read32(d: &pci::PciDevice, off: u8) -> u32 {
    crate::sync::without_interrupts(|| pci::read_config_u32(d.bus, d.slot, d.function, off))
}
fn cfg_write(d: &pci::PciDevice, off: u8, val: u32, size: u8) {
    crate::sync::without_interrupts(|| unsafe {
        let address = 0x8000_0000 | ((d.bus as u32) << 16) | ((d.slot as u32) << 11) | ((d.function as u32) << 8) | ((off as u32) & 0xfc);
        crate::port::outl(0xcf8, address);
        match size {
            1 => crate::port::outb(0xcfc + (off & 3) as u16, val as u8),
            2 => crate::port::outw(0xcfc + (off & 2) as u16, val as u16),
            _ => crate::port::outl(0xcfc, val),
        }
    });
}
fn read8(d: &pci::PciDevice, off: u8) -> u8 { (read32(d, off) >> ((off & 3) * 8)) as u8 }
fn handoff(index: usize, dev: &pci::PciDevice, eecp: u8) -> Result<()> {
    let Some(off) = e::find_legacy_support(eecp, |o| read8(dev, o)) else { return Ok(()) };
    let v = read32(dev, off);
    if v & e::LEGSUP_BIOS_OWNED != 0 {
        cfg_write(dev, off + 3, 1, 1);
        let d = Deadline::after_ms(1_000);
        while read32(dev, off) & e::LEGSUP_BIOS_OWNED != 0 {
            if d.expired() { return Err(EhciError::Timeout("firmware ownership")); }
            timer::sleep_ms(10);
        }
    }
    cfg_write(dev, off + 3, 1, 1);
    cfg_write(dev, off + 4, e::LEGCTLSTS_QUIET, 4);
    kprintln!("[ehci] controller {} firmware handoff done", index);
    Ok(())
}
static DEVICES: IrqSpinLock<Vec<pci::PciDevice>> = IrqSpinLock::new(Vec::new());
pub fn init() {
    pci::visit_devices_by_class(0x0c, |d| {
        let class = read32(&d, 0x08);
        if (class >> 8) & 0xffff == 0x0320 { DEVICES.lock().push(d); }
    });
    for (i, entry) in [worker0 as extern "C" fn(), worker1, worker2, worker3].iter().enumerate() {
        if i < DEVICES.lock().len() { let _ = sched::spawn("usb-ehci", *entry); }
    }
}
fn worker(index: usize) {
    let dev = { DEVICES.lock().get(index).copied() };
    if let Some(dev) = dev {
        match Ehci::new(index, &dev) {
            Ok(mut h) => h.run(),
            Err(e) => kprintln!("[ehci] controller {} unavailable: {}", index, e),
        }
    }
}
extern "C" fn worker0() { worker(0) }
extern "C" fn worker1() { worker(1) }
extern "C" fn worker2() { worker(2) }
extern "C" fn worker3() { worker(3) }

impl Ehci {
    fn new(index: usize, dev: &pci::PciDevice) -> Result<Ehci> {
        let bar = read32(dev, 0x10);
        if bar & 1 != 0 || bar & 6 == 4 || bar & 0xffff_fff0 < 0x4000_0000 {
            return Err(EhciError::Bar("unsupported or unassigned BAR0"));
        }
        let base = (bar & 0xffff_fff0) as u64;
        let virt = base;
        let command = read32(dev, 4) as u16;
        cfg_write(dev, 4, (command | 0x0406) as u32, 2);
        // SAFETY: the capability registers at the start of the mapped BAR.
        let r = |o: u64| unsafe { read_volatile((virt + o) as *const u32) };
        let caps = Capabilities::parse(r(e::CAPLENGTH), r(e::HCSPARAMS), r(e::HCCPARAMS)).ok_or(EhciError::Bar("no EHCI capability registers"))?;
        handoff(index, dev, caps.eecp)?;
        let mut h = Ehci {
            index,
            op: virt + caps.op_offset,
            caps,
            frame_list: Frame::new()?,
            heads: Frame::new()?,
            ctl: Frame::new()?,
            chain: Vec::new(),
            devices: BTreeMap::new(),
            root: BTreeMap::new(),
            used: [false; 128],
        };
        h.used[0] = true;
        h.start()?;
        kprintln!(
            "ehci{}: {:04x}:{:04x} at {}, version {:x}.{:02x}, {} ports, {} companion controller(s), {}-bit, BAR0 {:#x}",
            index,
            dev.vendor_id,
            dev.device_id,
            alloc::format!("{:02x}:{:02x}.{}", dev.bus, dev.slot, dev.function),
            caps.version >> 8,
            caps.version & 0xFF,
            caps.ports,
            caps.companions,
            if caps.ac64 { 64 } else { 32 },
            base
        );
        set_status(index, alloc::format!("ehci{}: running, {} ports", index, caps.ports));
        Ok(h)
    }

    fn r(&self, off: u64) -> u32 {
        // SAFETY: an operational register inside the mapped BAR.
        unsafe { read_volatile((self.op + off) as *const u32) }
    }

    fn w(&self, off: u64, v: u32) {
        // SAFETY: as in `r`.
        unsafe { write_volatile((self.op + off) as *mut u32, v) }
    }

    /// Stops, resets and starts the controller with empty schedules; routes
    /// every port to it and powers them.
    fn start(&mut self) -> Result<()> {
        self.w(op::USBCMD, self.r(op::USBCMD) & !usbcmd::RUN);
        let d = Deadline::after_ms(20);
        while self.r(op::USBSTS) & usbsts::HALTED == 0 && !d.expired() {
            core::hint::spin_loop();
        }
        self.w(op::USBCMD, usbcmd::HCRESET);
        let d = Deadline::after_ms(250);
        while self.r(op::USBCMD) & usbcmd::HCRESET != 0 {
            if d.expired() {
                return Err(EhciError::Timeout("controller reset"));
            }
            core::hint::spin_loop();
        }
        if self.caps.ac64 {
            self.w(op::CTRLDSSEGMENT, 0);
        }
        self.w(op::USBINTR, 0);
        self.w(op::USBSTS, usbsts::ACK);
        // The reclamation head points to itself and never executes; the
        // periodic anchor is parked too, and every frame starts at it.
        let parked = Endpoint { address: 0, number: 0, speed: Speed::High, max_packet: 64, control: false, tt: None };
        let head = e::queue_head(e::qh_link(self.heads.p32(HEAD_QH)), &parked, LINK_TERMINATE, true, false);
        self.heads.write(HEAD_QH, &head);
        self.heads.w32(HEAD_QH + QH_TOKEN, e::QH_PARKED);
        let anchor = e::queue_head(LINK_TERMINATE, &parked, LINK_TERMINATE, false, true);
        self.heads.write(ANCHOR_QH, &anchor);
        self.heads.w32(ANCHOR_QH + QH_TOKEN, e::QH_PARKED);
        let anchor_link = e::qh_link(self.heads.p32(ANCHOR_QH));
        for i in 0..1024 {
            self.frame_list.w32(4 * i, anchor_link);
        }
        fence(Ordering::SeqCst);
        self.w(op::PERIODICLISTBASE, self.frame_list.p32(0));
        self.w(op::ASYNCLISTADDR, self.heads.p32(HEAD_QH));
        self.w(op::FRINDEX, 0);
        self.w(op::USBCMD, usbcmd::RUN | usbcmd::PERIODIC_ENABLE | usbcmd::ASYNC_ENABLE | usbcmd::ITC_1MS);
        let d = Deadline::after_ms(20);
        while self.r(op::USBSTS) & usbsts::HALTED != 0 {
            if d.expired() {
                return Err(EhciError::Timeout("controller start"));
            }
            core::hint::spin_loop();
        }
        // Every port to this controller rather than to a companion.
        self.w(op::CONFIGFLAG, 1);
        if self.caps.port_power_control {
            for p in 1..=self.caps.ports {
                let v = self.r(op::portsc(p));
                self.w(op::portsc(p), portsc::update(v, portsc::POWER, 0));
            }
        }
        Ok(())
    }

    // ---- transfers ----------------------------------------------------------------------

    /// Waits until the controller no longer uses queue heads unlinked from
    /// the asynchronous schedule (EHCI 4.8.2).
    fn async_advance(&self) {
        self.w(op::USBSTS, usbsts::IAA);
        self.w(op::USBCMD, self.r(op::USBCMD) | usbcmd::IAAD);
        let d = Deadline::after_ms(100);
        while self.r(op::USBSTS) & usbsts::IAA == 0 {
            if d.expired() {
                kprintln!("ehci{}: no async advance acknowledgement", self.index);
                break;
            }
            core::hint::spin_loop();
        }
        self.w(op::USBSTS, usbsts::IAA);
    }

    /// One control transfer on endpoint 0 of `ep`'s device. Returns the
    /// bytes of the data stage.
    fn control(&mut self, ep: Endpoint, setup: SetupPacket, data: &mut [u8]) -> Result<usize> {
        let len = (setup.length as usize).min(data.len()).min(CTL_MAX);
        let c = &self.ctl;
        c.copy_in(CTL_SETUP_BUF, &setup.to_u64().to_le_bytes());
        if !setup.is_in() {
            c.copy_in(CTL_DATA, &data[..len]);
        }
        let status_pid = if setup.is_in() && len > 0 { Pid::Out } else { Pid::In };
        c.write(CTL_STATUS_TD, &e::qtd(LINK_TERMINATE, LINK_TERMINATE, e::qtd_token(status_pid, 0, true, true), 0, 0));
        let data_pid = if setup.is_in() { Pid::In } else { Pid::Out };
        // A short IN packet continues with the status stage.
        c.write(
            CTL_DATA_TD,
            &e::qtd(c.p32(CTL_STATUS_TD), c.p32(CTL_STATUS_TD), e::qtd_token(data_pid, len as u32, true, false), c.p32(CTL_DATA), len as u32),
        );
        let after_setup = if len > 0 { c.p32(CTL_DATA_TD) } else { c.p32(CTL_STATUS_TD) };
        c.write(CTL_SETUP_TD, &e::qtd(after_setup, LINK_TERMINATE, e::qtd_token(Pid::Setup, 8, false, false), c.p32(CTL_SETUP_BUF), 8));
        let ep = Endpoint { number: 0, control: true, ..ep };
        let qh = e::queue_head(self.heads.r32(HEAD_QH), &ep, c.p32(CTL_SETUP_TD), false, false);
        c.write(CTL_QH, &qh);
        fence(Ordering::SeqCst);
        self.heads.w32(HEAD_QH, e::qh_link(c.p32(CTL_QH)));
        let tds = [CTL_SETUP_TD, CTL_DATA_TD, CTL_STATUS_TD];
        let d = Deadline::after_ms(CONTROL_TIMEOUT_MS);
        let outcome = loop {
            if let Some(t) = tds.iter().map(|o| c.r32(o + TD_TOKEN)).find(|t| token::halted(*t)) {
                let errors = token::BABBLE | token::TRANSACTION_ERROR | token::BUFFER_ERROR;
                break Err(if t & errors == 0 { EhciError::Stall } else { EhciError::Transfer(t) });
            }
            if c.r32(CTL_STATUS_TD + TD_TOKEN) & token::ACTIVE == 0 {
                break Ok(());
            }
            if d.expired() {
                break Err(EhciError::Timeout("control transfer"));
            }
            timer::sleep_ms(1);
        };
        // Out of the schedule again before the memory is reused.
        self.heads.w32(HEAD_QH, self.ctl.r32(CTL_QH));
        self.async_advance();
        outcome?;
        let c = &self.ctl;
        let done = if len > 0 { len - (token::remaining(c.r32(CTL_DATA_TD + TD_TOKEN)) as usize).min(len) } else { 0 };
        if setup.is_in() {
            c.copy_out(CTL_DATA, &mut data[..done]);
        }
        Ok(done)
    }

    fn control_out(&mut self, ep: Endpoint, setup: SetupPacket, data: &[u8]) -> Result<()> {
        let mut copy = data.to_vec();
        self.control(ep, setup, &mut copy).map(|_| ())
    }

    fn ep0(&self, address: u8) -> Option<Endpoint> {
        let d = self.devices.get(&address)?;
        Some(Endpoint { address, number: 0, speed: d.speed, max_packet: d.max_packet0, control: true, tt: d.tt })
    }

    /// Puts an interrupt IN endpoint on the periodic schedule.
    fn open_pipe(&mut self, ep: Endpoint, kind: PipeKind) -> Result<Pipe> {
        let frame = Frame::new()?;
        let len = (ep.max_packet as u32).clamp(1, PIPE_MAX);
        frame.write(PIPE_TD, &e::qtd(LINK_TERMINATE, LINK_TERMINATE, e::qtd_token(Pid::In, len, false, true), frame.p32(PIPE_BUF), len));
        let qh = e::queue_head(self.heads.r32(ANCHOR_QH), &ep, frame.p32(PIPE_TD), false, true);
        frame.write(PIPE_QH, &qh);
        fence(Ordering::SeqCst);
        self.heads.w32(ANCHOR_QH, e::qh_link(frame.p32(PIPE_QH)));
        self.chain.insert(0, frame.p32(PIPE_QH));
        Ok(Pipe { frame, len, kind, errors: 0 })
    }

    /// Takes an interrupt endpoint off the periodic schedule and frees it.
    fn close_pipe(&mut self, pipe: Pipe) {
        let phys = pipe.frame.p32(PIPE_QH);
        if let Some(i) = self.chain.iter().position(|&p| p == phys) {
            let next = pipe.frame.r32(PIPE_QH);
            if i == 0 {
                self.heads.w32(ANCHOR_QH, next);
            } else {
                let prev = phys_to_virt(self.chain[i - 1] as u64);
                // SAFETY: the horizontal link of the previous queue head in
                // the chain, a frame this driver owns.
                unsafe { write_volatile(prev as *mut u32, next) };
            }
            self.chain.remove(i);
        }
        if let PipeKind::Hid(f) = &pipe.kind {
            let _ = f;
        }
        // The controller may be in the middle of the frame that reached the
        // queue head: wait for the next ones before freeing it.
        timer::sleep_ms(2);
        drop(pipe);
    }

    /// Collects the reports that arrived and re-arms the endpoints.
    fn poll_pipes(&mut self) {
        for dev in self.devices.values_mut() {
            for pipe in dev.pipes.iter_mut() {
                let f = &pipe.frame;
                let t = f.r32(PIPE_TD + TD_TOKEN);
                if t & token::ACTIVE != 0 || pipe.errors >= MAX_PIPE_ERRORS {
                    continue;
                }
                let halted = token::halted(t);
                if halted {
                    pipe.errors += 1;
                    if pipe.errors == MAX_PIPE_ERRORS {
                        kprintln!("usb {}: interrupt endpoint keeps failing ({}); left alone", dev.name, EhciError::Transfer(t));
                        continue;
                    }
                } else {
                    let n = pipe.len.saturating_sub(token::remaining(t)) as usize;
                    let mut report = [0u8; PIPE_MAX as usize];
                    f.copy_out(PIPE_BUF, &mut report[..n]);
                    match &mut pipe.kind {
                        PipeKind::Hid(f) if n > 0 => {
                            let _ = f.report(&report[..n]);
                        }
                        PipeKind::Hub { pending } => {
                            *pending |= report[..n.min(4)].iter().enumerate().fold(0u32, |m, (i, b)| m | (*b as u32) << (8 * i));
                        }
                        _ => {}
                    }
                    pipe.errors = 0;
                }
                f.write(PIPE_TD, &e::qtd(LINK_TERMINATE, LINK_TERMINATE, e::qtd_token(Pid::In, pipe.len, false, true), f.p32(PIPE_BUF), pipe.len));
                fence(Ordering::SeqCst);
                if halted {
                    // The queue head is out of service: start it again
                    // with data toggle 0.
                    f.w32(PIPE_QH + QH_TOKEN, 0);
                }
                f.w32(PIPE_QH + QH_NEXT, f.p32(PIPE_TD));
            }
        }
    }

    // ---- devices ------------------------------------------------------------------------

    fn alloc_address(&mut self) -> Result<u8> {
        let a = (1..128).find(|&a| !self.used[a]).ok_or(EhciError::NoAddress)?;
        self.used[a] = true;
        Ok(a as u8)
    }

    /// Releases a device and, for a hub, every device behind it first.
    fn remove(&mut self, address: u8) {
        let Some(mut dev) = self.devices.remove(&address) else { return };
        if let Some(hub) = dev.hub.take() {
            for child in hub.children.into_values() {
                self.remove(child);
            }
        }
        for pipe in core::mem::take(&mut dev.pipes) {
            self.close_pipe(pipe);
        }
        self.used[address as usize] = false;
        kprintln!("usb {}: disconnected", dev.name);
    }

    /// Addresses and configures a device that was just reset; returns its
    /// address. `prefix` names where it is (`ehci0-1.3`).
    fn enumerate(&mut self, speed: Speed, tt: Option<(u8, u8)>, depth: u8, prefix: String) -> Result<u8> {
        let mut ep = Endpoint { address: 0, number: 0, speed, max_packet: speed.default_max_packet0(), control: true, tt };
        let mut head = [0u8; 8];
        let n = self.control(ep, SetupPacket::get_descriptor(descriptor::DESC_DEVICE, 0, 0, 8), &mut head)?;
        let mps = DeviceDescriptor::max_packet0_from_prefix(&head[..n])? as u16;
        if mps != 0 {
            ep.max_packet = mps;
        }
        let address = self.alloc_address()?;
        let set_address = SetupPacket { request_type: 0, request: descriptor::REQ_SET_ADDRESS, value: address as u16, index: 0, length: 0 };
        if let Err(err) = self.control_out(ep, set_address, &[]) {
            self.used[address as usize] = false;
            return Err(err);
        }
        timer::sleep_ms(RESET_RECOVERY_MS);
        self.devices.insert(
            address,
            Device { speed, max_packet0: ep.max_packet, tt, depth, name: alloc::format!("{prefix} (address {address})"), hub: None, pipes: Vec::new() },
        );
        match self.configure(address, speed, prefix) {
            Ok(()) => Ok(address),
            Err(err) => {
                self.remove(address);
                Err(err)
            }
        }
    }

    fn configure(&mut self, address: u8, speed: Speed, prefix: String) -> Result<()> {
        let ep = self.ep0(address).ok_or(EhciError::PortGone)?;
        let mut dd = [0u8; DeviceDescriptor::LEN];
        let n = self.control(ep, SetupPacket::get_descriptor(descriptor::DESC_DEVICE, 0, 0, DeviceDescriptor::LEN as u16), &mut dd)?;
        let desc = DeviceDescriptor::parse(&dd[..n])?;
        let mut c9 = [0u8; 9];
        let n = self.control(ep, SetupPacket::get_descriptor(descriptor::DESC_CONFIGURATION, 0, 0, 9), &mut c9)?;
        let total = descriptor::configuration_total_length(&c9[..n])? as usize;
        if total > CTL_MAX {
            return Err(EhciError::Unsupported("configuration descriptor larger than 2 KiB"));
        }
        let mut raw = vec![0u8; total];
        let n = self.control(ep, SetupPacket::get_descriptor(descriptor::DESC_CONFIGURATION, 0, 0, total as u16), &mut raw)?;
        let cfg = Configuration::parse(&raw[..n])?;
        let product = self.string(ep, desc.product_index).unwrap_or_default();
        let label = if product.is_empty() { alloc::format!("{:04x}:{:04x}", desc.vendor, desc.product) } else { product.clone() };
        let name = alloc::format!("{prefix} ({label})");
        kprintln!(
            "usb {}: {:04x}:{:04x} \"{}\", {}, USB {:x}.{:02x}, class {:02x}, {} interface(s), EP0 max packet {}, address {}",
            prefix,
            desc.vendor,
            desc.product,
            product,
            speed.name(),
            desc.usb_version >> 8,
            desc.usb_version & 0xFF,
            desc.class,
            cfg.interfaces.len(),
            ep.max_packet,
            address
        );
        if let Some(d) = self.devices.get_mut(&address) {
            d.name = name.clone();
        }
        if desc.class == descriptor::CLASS_HUB {
            let status = cfg.default_interfaces().find(|i| i.class == descriptor::CLASS_HUB).and_then(|i| i.interrupt_in()).copied();
            return self.setup_hub(address, cfg.value, status);
        }
        let hid: Vec<Interface> =
            cfg.default_interfaces().filter(|i| i.class == descriptor::CLASS_HID && i.interrupt_in().is_some()).cloned().collect();
        if hid.is_empty() {
            let class = match (desc.class, cfg.default_interfaces().next()) {
                (0, Some(i)) => i.class,
                _ => desc.class,
            };
            kprintln!("usb {}: class {:02x} is not driven on EHCI ports (only HID and hubs); left unconfigured", name, class);
            return Ok(());
        }
        self.control_out(ep, SetupPacket::set_configuration(cfg.value), &[])?;
        for iface in &hid {
            let function = match usbhid::setup(&mut DevicePipe { ehci: self, ep }, iface, &label, &name) {
                Ok(Some(f)) => f,
                Ok(None) => continue,
                Err(err) => {
                    kprintln!("usb {}: interface {}: {}", name, iface.number, err);
                    continue;
                }
            };
            let int = *iface.interrupt_in().expect("filtered for an interrupt IN endpoint");
            let pipe_ep = Endpoint { number: int.number(), max_packet: int.max_packet.max(1), control: false, ..ep };
            match self.open_pipe(pipe_ep, PipeKind::Hid(Box::new(function))) {
                Ok(pipe) => {
                    kprintln!("usb {}: endpoint {:#04x}: interrupt IN, {} bytes, polled every 1 ms", name, int.address, pipe.len);
                    if let Some(d) = self.devices.get_mut(&address) {
                        d.pipes.push(pipe);
                    }
                }
                Err(err) => kprintln!("usb {}: interface {}: {}", name, iface.number, err),
            }
        }
        Ok(())
    }

    fn string(&mut self, ep: Endpoint, index: u8) -> Option<String> {
        if index == 0 {
            return None;
        }
        let mut langs = [0u8; 4];
        let lang = self
            .control(ep, SetupPacket::get_descriptor(descriptor::DESC_STRING, 0, 0, 4), &mut langs)
            .ok()
            .and_then(|n| descriptor::first_language(&langs[..n]))
            .unwrap_or(0x0409);
        let mut buf = [0u8; 255];
        let n = self.control(ep, SetupPacket::get_descriptor(descriptor::DESC_STRING, index, lang, 255), &mut buf).ok()?;
        descriptor::parse_string(&buf[..n]).ok()
    }

    // ---- root ports ---------------------------------------------------------------------

    fn scan_root(&mut self, initial: bool) {
        for p in 1..=self.caps.ports {
            let v = self.r(op::portsc(p));
            if v & portsc::OWNER != 0 {
                continue;
            }
            let changed = v & portsc::CONNECT_CHANGE != 0;
            if !changed && !initial {
                continue;
            }
            self.w(op::portsc(p), portsc::acknowledge(v));
            if let Some(address) = self.root.remove(&p) {
                self.remove(address);
            }
            if v & portsc::CONNECTED == 0 {
                continue;
            }
            for attempt in 1..=ATTACH_ATTEMPTS {
                match self.attach_root(p) {
                    Ok(Some(address)) => {
                        self.root.insert(p, address);
                        break;
                    }
                    Ok(None) | Err(EhciError::PortGone) => break,
                    Err(err) if attempt == ATTACH_ATTEMPTS => kprintln!("ehci{}: port {}: device not usable: {}", self.index, p, err),
                    Err(err) => kprintln!("ehci{}: port {}: attempt {} failed: {}; resetting the port again", self.index, p, attempt, err),
                }
            }
        }
        self.report_status();
    }

    /// Resets a root port and enumerates its device. `None`: a low or
    /// full-speed device, which only a companion controller can drive.
    fn attach_root(&mut self, p: u8) -> Result<Option<u8>> {
        timer::sleep_ms(DEBOUNCE_MS);
        let v = self.r(op::portsc(p));
        if v & portsc::CONNECTED == 0 {
            return Err(EhciError::PortGone);
        }
        if v & portsc::LINE_STATUS == portsc::LINE_K {
            self.release(p, "low-speed");
            return Ok(None);
        }
        self.w(op::portsc(p), portsc::update(v, portsc::RESET, portsc::ENABLED));
        timer::sleep_ms(ROOT_RESET_MS);
        let v = self.r(op::portsc(p));
        self.w(op::portsc(p), portsc::update(v, 0, portsc::RESET));
        let d = Deadline::after_ms(50);
        while self.r(op::portsc(p)) & portsc::RESET != 0 {
            if d.expired() {
                return Err(EhciError::Timeout("port reset"));
            }
            core::hint::spin_loop();
        }
        timer::sleep_ms(2);
        let v = self.r(op::portsc(p));
        if v & portsc::CONNECTED == 0 {
            return Err(EhciError::PortGone);
        }
        if v & portsc::ENABLED == 0 {
            self.release(p, "full-speed");
            return Ok(None);
        }
        timer::sleep_ms(RESET_RECOVERY_MS);
        kprintln!("ehci{}: port {}: high-speed device connected", self.index, p);
        self.enumerate(Speed::High, None, 0, alloc::format!("ehci{}-{}", self.index, p)).map(Some)
    }

    /// A low or full-speed device on a root port: a companion controller's
    /// (OSyx drives none), or nobody's on a controller without one.
    fn release(&mut self, p: u8, speed: &str) {
        if self.caps.companions > 0 {
            let v = self.r(op::portsc(p));
            self.w(op::portsc(p), portsc::update(v, portsc::OWNER, 0));
            kprintln!("ehci{}: port {}: {} device handed to the companion controller (not driven by OSyx)", self.index, p, speed);
        } else {
            kprintln!("ehci{}: port {}: {} device on a port without a companion controller: not usable", self.index, p, speed);
        }
    }

    // ---- hubs ---------------------------------------------------------------------------

    fn setup_hub(&mut self, address: u8, config: u8, status: Option<descriptor::EndpointDescriptor>) -> Result<()> {
        let ep = self.ep0(address).ok_or(EhciError::PortGone)?;
        if self.devices[&address].depth >= MAX_DEPTH {
            return Err(EhciError::Unsupported("more than five tiers of hubs"));
        }
        self.control_out(ep, SetupPacket::set_configuration(config), &[])?;
        let mut d = [0u8; 9];
        let n = self.control(ep, hubreq::hub_descriptor(), &mut d)?;
        if n < 7 || d[1] != 0x29 {
            return Err(EhciError::Unsupported("hub descriptor"));
        }
        let ports = d[2].min(15);
        let power_on_ms = d[5] as u64 * 2;
        let name = self.devices[&address].name.clone();
        kprintln!("usb {}: hub with {} ports", name, ports);
        for p in 1..=ports {
            self.control_out(ep, hubreq::set_port(p, hubreq::PORT_POWER), &[])?;
        }
        timer::sleep_ms(power_on_ms.max(20) + DEBOUNCE_MS);
        // The status change endpoint, opened before the first scan so that
        // nothing that changes during the scan is missed.
        let pipe = match status {
            Some(int) => {
                let pipe_ep = Endpoint { number: int.number(), max_packet: int.max_packet.clamp(1, 4), control: false, ..ep };
                match self.open_pipe(pipe_ep, PipeKind::Hub { pending: 0 }) {
                    Ok(p) => Some(p),
                    Err(err) => {
                        kprintln!("usb {}: status change endpoint: {}; asking the hub every {} ms", name, err, PORT_POLL_MS);
                        None
                    }
                }
            }
            None => None,
        };
        if let Some(dev) = self.devices.get_mut(&address) {
            dev.hub = Some(Hub { ports, notifies: pipe.is_some(), children: BTreeMap::new() });
            dev.pipes.extend(pipe);
        }
        for p in 1..=ports {
            self.hub_port(address, p, true);
        }
        Ok(())
    }

    fn hub_port_status(&mut self, hub: u8, port: u8) -> Option<(u16, u16)> {
        let ep = self.ep0(hub)?;
        let mut st = [0u8; 4];
        match self.control(ep, hubreq::port_status(port), &mut st) {
            Ok(4) => Some((u16::from_le_bytes([st[0], st[1]]), u16::from_le_bytes([st[2], st[3]]))),
            _ => None,
        }
    }

    /// Handles a connect or disconnect on a hub port (`initial`: the scan
    /// right after the hub was configured).
    fn hub_port(&mut self, hub: u8, port: u8, initial: bool) {
        let Some((status, change)) = self.hub_port_status(hub, port) else { return };
        let Some(ep) = self.ep0(hub) else { return };
        // Every change is acknowledged, or the hub keeps reporting it.
        for bit in 0..5 {
            if change & (1 << bit) != 0 {
                let _ = self.control_out(ep, hubreq::clear_port(port, hubreq::C_PORT_CONNECTION + bit), &[]);
            }
        }
        if !initial && change & hubreq::CHANGE_CONNECTION == 0 {
            return;
        }
        let existing = self.devices.get_mut(&hub).and_then(|d| d.hub.as_mut()).and_then(|h| h.children.remove(&port));
        if let Some(child) = existing {
            self.remove(child);
        }
        if status & hubreq::CONNECTION == 0 {
            return;
        }
        let hub_name = self.devices.get(&hub).map_or(String::new(), |d| d.name.clone());
        for attempt in 1..=ATTACH_ATTEMPTS {
            match self.attach_behind_hub(hub, port) {
                Ok(child) => {
                    if let Some(h) = self.devices.get_mut(&hub).and_then(|d| d.hub.as_mut()) {
                        h.children.insert(port, child);
                    }
                    break;
                }
                Err(EhciError::PortGone) => break,
                Err(err) if attempt == ATTACH_ATTEMPTS => kprintln!("usb {}: port {}: device not usable: {}", hub_name, port, err),
                Err(err) => kprintln!("usb {}: port {}: attempt {} failed: {}; resetting the port again", hub_name, port, attempt, err),
            }
        }
    }

    fn attach_behind_hub(&mut self, hub: u8, port: u8) -> Result<u8> {
        let ep = self.ep0(hub).ok_or(EhciError::PortGone)?;
        timer::sleep_ms(DEBOUNCE_MS);
        self.control_out(ep, hubreq::set_port(port, hubreq::PORT_RESET), &[])?;
        let mut status = 0;
        let mut done = false;
        for _ in 0..50 {
            timer::sleep_ms(10);
            if let Some((s, c)) = self.hub_port_status(hub, port) {
                status = s;
                if c & hubreq::CHANGE_RESET != 0 {
                    done = true;
                    break;
                }
            }
        }
        let _ = self.control_out(ep, hubreq::clear_port(port, hubreq::C_PORT_RESET), &[]);
        let _ = self.control_out(ep, hubreq::clear_port(port, hubreq::C_PORT_ENABLE), &[]);
        if !done {
            return Err(EhciError::Timeout("hub port reset"));
        }
        if status & hubreq::CONNECTION == 0 {
            return Err(EhciError::PortGone);
        }
        if status & hubreq::ENABLE == 0 {
            return Err(EhciError::Unsupported("hub port not enabled after reset"));
        }
        timer::sleep_ms(RESET_RECOVERY_MS);
        let speed = if status & hubreq::LOW_SPEED != 0 {
            Speed::Low
        } else if status & hubreq::HIGH_SPEED != 0 {
            Speed::High
        } else {
            Speed::Full
        };
        let parent = &self.devices[&hub];
        // The nearest high-speed hub translates for low and full-speed devices.
        let tt = if parent.speed == Speed::High && speed != Speed::High { Some((hub, port)) } else { parent.tt };
        let depth = parent.depth + 1;
        let prefix = match parent.name.split_once(' ') {
            Some((place, _)) => alloc::format!("{place}.{port}"),
            None => alloc::format!("{}.{port}", parent.name),
        };
        kprintln!("usb {}: {} device connected", prefix, speed.name());
        self.enumerate(speed, tt, depth, prefix)
    }

    /// Handles the ports hubs reported changes on, and asks the hubs that
    /// have no status change endpoint (`all`: the periodic round).
    fn poll_hubs(&mut self, all: bool) {
        let mut hubs: Vec<(u8, u32)> = Vec::new();
        for (address, dev) in self.devices.iter_mut() {
            let Some(hub) = dev.hub.as_ref() else { continue };
            let mut ports = 0u32;
            for pipe in dev.pipes.iter_mut() {
                if let PipeKind::Hub { pending } = &mut pipe.kind {
                    ports |= core::mem::take(pending);
                }
            }
            if !hub.notifies && all {
                ports = u32::MAX;
            }
            let mask = ((1u32 << (hub.ports + 1)) - 1) & !1;
            if ports & mask != 0 {
                hubs.push((*address, ports & mask));
            }
        }
        for (address, ports) in hubs {
            for p in (1..32u8).filter(|p| ports & (1 << p) != 0) {
                // A hub removed by an earlier port of this round is gone.
                if !self.devices.contains_key(&address) {
                    break;
                }
                self.hub_port(address, p, false);
            }
        }
    }

    fn report_status(&self) {
        let functions: usize = self.devices.values().map(|d| d.pipes.iter().filter(|p| matches!(p.kind, PipeKind::Hid(_))).count()).sum();
        set_status(
            self.index,
            alloc::format!("ehci{}: running, {} devices, {} HID functions", self.index, self.devices.len(), functions),
        );
    }

    fn run(&mut self) -> ! {
        // Give devices time to come up after the ports were powered.
        timer::sleep_ms(DEBOUNCE_MS);
        self.scan_root(true);
        let mut last_ports = crate::timer::uptime_ms();
        loop {
            let st = self.r(op::USBSTS);
            if st & usbsts::HOST_SYSTEM_ERROR != 0 {
                kprintln!("ehci{}: host system error (USBSTS {:#x}); the controller stopped", self.index, st);
                set_status(self.index, alloc::format!("ehci{}: stopped after a host system error", self.index));
                loop {
                    timer::sleep_ms(60_000);
                }
            }
            self.w(op::USBSTS, st & (usbsts::USBINT | usbsts::USBERRINT | usbsts::FRAME_ROLLOVER));
            self.poll_pipes();
            self.poll_hubs(false);

            let now = crate::timer::uptime_ms();
            if st & usbsts::PORT_CHANGE != 0 || now.saturating_sub(last_ports) >= PORT_POLL_MS {
                self.w(op::USBSTS, usbsts::PORT_CHANGE);
                self.scan_root(false);
                self.poll_hubs(true);
                last_ports = crate::timer::uptime_ms();
            }
            timer::sleep_ms(1);
        }
    }
}

/// Endpoint 0 of a device for code shared with other host controller drivers.
struct DevicePipe<'a> {
    ehci: &'a mut Ehci,
    ep: Endpoint,
}

impl ControlPipe for DevicePipe<'_> {
    type Error = EhciError;
    fn control_in(&mut self, setup: SetupPacket, buf: &mut [u8]) -> Result<usize> {
        self.ehci.control(self.ep, setup, buf)
    }
    fn control_out(&mut self, setup: SetupPacket, data: &[u8]) -> Result<()> {
        self.ehci.control_out(self.ep, setup, data)
    }
}

/// Hub class requests and port features (USB 2.0 chapter 11).
mod hubreq {
    use crate::usb2::descriptor::SetupPacket;
    pub const PORT_RESET: u16 = 4;
    pub const PORT_POWER: u16 = 8;
    pub const C_PORT_CONNECTION: u16 = 16;
    pub const C_PORT_ENABLE: u16 = 17;
    pub const C_PORT_RESET: u16 = 20;
    pub const CONNECTION: u16 = 1 << 0;
    pub const ENABLE: u16 = 1 << 1;
    pub const LOW_SPEED: u16 = 1 << 9;
    pub const HIGH_SPEED: u16 = 1 << 10;
    pub const CHANGE_CONNECTION: u16 = 1 << 0;
    pub const CHANGE_RESET: u16 = 1 << 4;

    pub fn hub_descriptor() -> SetupPacket {
        SetupPacket { request_type: 0xA0, request: 6, value: 0x2900, index: 0, length: 9 }
    }
    pub fn port_status(port: u8) -> SetupPacket {
        SetupPacket { request_type: 0xA3, request: 0, value: 0, index: port as u16, length: 4 }
    }
    pub fn set_port(port: u8, feature: u16) -> SetupPacket {
        SetupPacket { request_type: 0x23, request: 3, value: feature, index: port as u16, length: 0 }
    }
    pub fn clear_port(port: u8, feature: u16) -> SetupPacket {
        SetupPacket { request_type: 0x23, request: 1, value: feature, index: port as u16, length: 0 }
    }
}
