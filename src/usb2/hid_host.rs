//! USB HID boot-mouse class support over the EHCI control/interrupt pipes.
use super::descriptor::{self, Interface, SetupPacket};
use super::hid::{self, MouseLayout};
use alloc::vec;
use core::sync::atomic::{AtomicBool, Ordering};
static FIRST_MOVEMENT_LOGGED: AtomicBool = AtomicBool::new(false);
static FIRST_KEYBOARD_LOGGED: AtomicBool = AtomicBool::new(false);

enum Kind { Mouse(Option<MouseLayout>), Keyboard(crate::usb_keyboard::Keyboard) }
pub struct HidFunction { kind: Kind }
impl HidFunction {
    pub fn report(&mut self, data: &[u8]) -> Result<(), ()> {
        match &mut self.kind {
            Kind::Keyboard(keyboard) => {
                let mut events = alloc::vec::Vec::new();
                keyboard.report(data, &mut events)?;
                for event in events { crate::keyboard::usb_input(event); }
                if !FIRST_KEYBOARD_LOGGED.swap(true, Ordering::Relaxed) {
                    crate::serial_println!("[usb] keyboard report received");
                }
            }
            Kind::Mouse(layout) => {
                let r = match layout { Some(layout) => layout.decode(data), None => hid::boot_mouse(data) }.map_err(|_| ())?;
                crate::mouse::usb_report(r.dx, r.dy, r.buttons);
                if (r.dx != 0 || r.dy != 0 || r.buttons != 0)
                    && !FIRST_MOVEMENT_LOGGED.swap(true, Ordering::Relaxed) {
                    crate::serial_println!("[usb] mouse report applied");
                }
            }
        }
        Ok(())
    }
}
pub trait ControlPipe {
    type Error: core::fmt::Display;
    fn control_in(&mut self, setup: SetupPacket, buf: &mut [u8]) -> Result<usize, Self::Error>;
    fn control_out(&mut self, setup: SetupPacket, data: &[u8]) -> Result<(), Self::Error>;
}
pub fn setup<P: ControlPipe>(pipe: &mut P, iface: &Interface, _name: &str, devname: &str) -> Result<Option<HidFunction>, P::Error> {
    if iface.is_hid_boot(descriptor::HID_PROTOCOL_KEYBOARD) {
        pipe.control_out(SetupPacket::hid_set_protocol(iface.number, false), &[])?;
        let _ = pipe.control_out(SetupPacket::hid_set_idle(iface.number, 0), &[]);
        crate::serial_println!("[usb] boot keyboard {} interface {} ready", devname, iface.number);
        return Ok(Some(HidFunction { kind: Kind::Keyboard(crate::usb_keyboard::Keyboard::new()) }));
    }
    let is_boot = iface.is_hid_boot(descriptor::HID_PROTOCOL_MOUSE);
    let report_len = iface.hid.map_or(0, |h| h.report_length).min(1024);
    let mut layout = None;
    if report_len > 0 {
        let mut bytes = vec![0u8; report_len as usize];
        if let Ok(n) = pipe.control_in(
            SetupPacket::get_interface_descriptor(descriptor::DESC_HID_REPORT, iface.number, report_len),
            &mut bytes,
        ) {
            if n <= bytes.len() {
                if let Ok(fields) = hid::parse_report_descriptor(&bytes[..n]) {
                    layout = MouseLayout::from_fields(&fields);
                }
            }
        }
    }
    // Only boot-subclass mice accept SET_PROTOCOL. Report-only devices do
    // not need this class request; their layout came from their descriptor.
    if is_boot {
        pipe.control_out(SetupPacket::hid_set_protocol(iface.number, layout.is_some()), &[])?;
    } else if layout.is_none() {
        return Ok(None);
    }
    let _ = pipe.control_out(SetupPacket::hid_set_idle(iface.number, 0), &[]);
    crate::serial_println!("[usb] boot mouse {} interface {} ready", devname, iface.number);
    Ok(Some(HidFunction { kind: Kind::Mouse(layout) }))
}
