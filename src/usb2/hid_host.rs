//! USB HID boot-mouse class support over the EHCI control/interrupt pipes.
use super::descriptor::{self, Interface, SetupPacket};
use crate::usb_hid_mouse::parse_boot_mouse_report;

pub struct HidFunction { pub input_id: u32 }
impl HidFunction {
    pub fn report(&mut self, data: &[u8]) -> Result<(), ()> {
        let r = parse_boot_mouse_report(data).ok_or(())?;
        crate::mouse::usb_report(r.dx, r.dy, r.buttons);
        Ok(())
    }
}
pub trait ControlPipe {
    type Error: core::fmt::Display;
    fn control_in(&mut self, setup: SetupPacket, buf: &mut [u8]) -> Result<usize, Self::Error>;
    fn control_out(&mut self, setup: SetupPacket, data: &[u8]) -> Result<(), Self::Error>;
}
pub fn setup<P: ControlPipe>(pipe: &mut P, iface: &Interface, _name: &str, devname: &str) -> Result<Option<HidFunction>, P::Error> {
    if !iface.is_hid_boot(descriptor::HID_PROTOCOL_MOUSE) { return Ok(None); }
    pipe.control_out(SetupPacket::hid_set_protocol(iface.number, false), &[])?;
    let _ = pipe.control_out(SetupPacket::hid_set_idle(iface.number, 0), &[]);
    crate::serial_println!("[usb] boot mouse {} interface {} ready", devname, iface.number);
    Ok(Some(HidFunction { input_id: 0 }))
}
