//! Standard USB requests and descriptors (USB 2.0 chapter 9, HID 1.11
//! section 6.2.1).
//!
//! Descriptor data comes from the device and is untrusted: every parser
//! checks `bLength`, the descriptor type and the bounds of the buffer, and
//! a configuration descriptor tree is rejected as a whole if any entry is
//! malformed.

use alloc::string::String;
use alloc::vec::Vec;

pub const DESC_DEVICE: u8 = 1;
pub const DESC_CONFIGURATION: u8 = 2;
pub const DESC_STRING: u8 = 3;
pub const DESC_INTERFACE: u8 = 4;
pub const DESC_ENDPOINT: u8 = 5;
pub const DESC_HID: u8 = 0x21;
pub const DESC_HID_REPORT: u8 = 0x22;
pub const DESC_SS_ENDPOINT_COMPANION: u8 = 0x30;

pub const REQ_GET_STATUS: u8 = 0;
pub const REQ_SET_ADDRESS: u8 = 5;
pub const REQ_GET_DESCRIPTOR: u8 = 6;
pub const REQ_SET_CONFIGURATION: u8 = 9;
pub const HID_GET_REPORT: u8 = 0x01;
pub const HID_SET_REPORT: u8 = 0x09;
pub const HID_SET_IDLE: u8 = 0x0A;
pub const HID_SET_PROTOCOL: u8 = 0x0B;

pub const CLASS_HID: u8 = 3;
pub const CLASS_MASS_STORAGE: u8 = 8;
pub const CLASS_HUB: u8 = 9;
pub const MASS_STORAGE_SCSI: u8 = 6;
pub const MASS_STORAGE_BOT: u8 = 0x50;
pub const HID_SUBCLASS_BOOT: u8 = 1;
pub const HID_PROTOCOL_KEYBOARD: u8 = 1;
pub const HID_PROTOCOL_MOUSE: u8 = 2;

/// The 8-byte SETUP packet of a control transfer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetupPacket {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
}

impl SetupPacket {
    pub fn is_in(&self) -> bool {
        self.request_type & 0x80 != 0
    }

    /// Little-endian packing used as the Setup Stage TRB's immediate data.
    pub fn to_u64(&self) -> u64 {
        self.request_type as u64
            | (self.request as u64) << 8
            | (self.value as u64) << 16
            | (self.index as u64) << 32
            | (self.length as u64) << 48
    }

    /// GET_DESCRIPTOR (device recipient).
    pub fn get_descriptor(kind: u8, index: u8, language: u16, length: u16) -> SetupPacket {
        SetupPacket { request_type: 0x80, request: REQ_GET_DESCRIPTOR, value: (kind as u16) << 8 | index as u16, index: language, length }
    }

    /// GET_DESCRIPTOR of a class descriptor addressed to an interface (the
    /// HID report descriptor).
    pub fn get_interface_descriptor(kind: u8, interface: u8, length: u16) -> SetupPacket {
        SetupPacket { request_type: 0x81, request: REQ_GET_DESCRIPTOR, value: (kind as u16) << 8, index: interface as u16, length }
    }

    pub fn set_configuration(value: u8) -> SetupPacket {
        SetupPacket { request_type: 0x00, request: REQ_SET_CONFIGURATION, value: value as u16, index: 0, length: 0 }
    }

    /// HID SET_PROTOCOL: 0 = boot protocol, 1 = report protocol.
    pub fn hid_set_protocol(interface: u8, report_protocol: bool) -> SetupPacket {
        SetupPacket { request_type: 0x21, request: HID_SET_PROTOCOL, value: u16::from(report_protocol), index: interface as u16, length: 0 }
    }

    /// HID SET_IDLE: `duration` in 4 ms units, 0 = report only on change.
    pub fn hid_set_idle(interface: u8, duration: u8) -> SetupPacket {
        SetupPacket { request_type: 0x21, request: HID_SET_IDLE, value: (duration as u16) << 8, index: interface as u16, length: 0 }
    }

    /// HID SET_REPORT of an output report (keyboard LEDs).
    pub fn hid_set_output_report(interface: u8, report_id: u8, length: u16) -> SetupPacket {
        SetupPacket { request_type: 0x21, request: HID_SET_REPORT, value: 0x0200 | report_id as u16, index: interface as u16, length }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DescriptorError {
    /// Shorter than the fixed part of the descriptor.
    Truncated,
    /// `bDescriptorType` is not what was requested.
    WrongType,
    /// `bLength` is smaller than the descriptor's minimum, or runs past the
    /// end of the data.
    BadLength,
    /// `wTotalLength` does not match the data, or the tree is inconsistent.
    BadTotalLength,
    /// An endpoint descriptor appears outside any interface, an interface
    /// has more endpoints than announced, or similar structural damage.
    BadStructure,
    TooManyEntries,
}

fn u16_at(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

/// Device descriptor (18 bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceDescriptor {
    pub usb_version: u16,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub max_packet0: u8,
    pub vendor: u16,
    pub product: u16,
    pub device_version: u16,
    pub manufacturer_index: u8,
    pub product_index: u8,
    pub serial_index: u8,
    pub num_configurations: u8,
}

impl DeviceDescriptor {
    pub const LEN: usize = 18;

    pub fn parse(b: &[u8]) -> Result<DeviceDescriptor, DescriptorError> {
        if b.len() < 8 {
            return Err(DescriptorError::Truncated);
        }
        if b[1] != DESC_DEVICE {
            return Err(DescriptorError::WrongType);
        }
        if (b[0] as usize) < Self::LEN {
            return Err(DescriptorError::BadLength);
        }
        if b.len() < Self::LEN {
            return Err(DescriptorError::Truncated);
        }
        Ok(DeviceDescriptor {
            usb_version: u16_at(b, 2),
            class: b[4],
            subclass: b[5],
            protocol: b[6],
            max_packet0: b[7],
            vendor: u16_at(b, 8),
            product: u16_at(b, 10),
            device_version: u16_at(b, 12),
            manufacturer_index: b[14],
            product_index: b[15],
            serial_index: b[16],
            num_configurations: b[17],
        })
    }

    /// `bMaxPacketSize0` from the first 8 bytes alone (read before the full
    /// descriptor to fix up the default endpoint).
    pub fn max_packet0_from_prefix(b: &[u8]) -> Result<u8, DescriptorError> {
        if b.len() < 8 {
            return Err(DescriptorError::Truncated);
        }
        if b[1] != DESC_DEVICE {
            return Err(DescriptorError::WrongType);
        }
        Ok(b[7])
    }

    /// Effective max packet size of EP0: for USB 3 devices `bMaxPacketSize0`
    /// is an exponent (9 -> 512).
    pub fn ep0_max_packet(&self) -> u16 {
        if self.usb_version >= 0x0300 {
            1u16 << self.max_packet0.min(15)
        } else {
            self.max_packet0 as u16
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EndpointDescriptor {
    pub address: u8,
    pub attributes: u8,
    /// Bits 10:0 of wMaxPacketSize.
    pub max_packet: u16,
    /// Additional transactions per microframe (high-speed, bits 12:11).
    pub mult: u8,
    pub interval: u8,
    /// From a SuperSpeed endpoint companion, if present.
    pub max_burst: u8,
    pub bytes_per_interval: u16,
}

impl EndpointDescriptor {
    pub fn is_in(&self) -> bool {
        self.address & 0x80 != 0
    }

    pub fn number(&self) -> u8 {
        self.address & 0x0F
    }

    pub fn is_interrupt(&self) -> bool {
        self.attributes & 3 == 3
    }
}

/// The HID class descriptor of an interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HidDescriptor {
    pub hid_version: u16,
    pub country: u8,
    /// Length of the report descriptor (`wDescriptorLength` of the entry
    /// with type 0x22), 0 if none was announced.
    pub report_length: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interface {
    pub number: u8,
    pub alternate: u8,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub name_index: u8,
    pub hid: Option<HidDescriptor>,
    pub endpoints: Vec<EndpointDescriptor>,
}

impl Interface {
    pub fn is_hid_boot(&self, protocol: u8) -> bool {
        self.class == CLASS_HID && self.subclass == HID_SUBCLASS_BOOT && self.protocol == protocol
    }

    /// The first interrupt IN endpoint.
    pub fn interrupt_in(&self) -> Option<&EndpointDescriptor> {
        self.endpoints.iter().find(|e| e.is_in() && e.is_interrupt())
    }

    /// The first bulk IN endpoint.
    pub fn bulk_in(&self) -> Option<&EndpointDescriptor> {
        self.endpoints.iter().find(|e| e.is_in() && e.attributes & 3 == 2)
    }

    /// The first bulk OUT endpoint.
    pub fn bulk_out(&self) -> Option<&EndpointDescriptor> {
        self.endpoints.iter().find(|e| !e.is_in() && e.attributes & 3 == 2)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Configuration {
    pub value: u8,
    pub attributes: u8,
    /// Maximum power in mA (2 mA units for USB 2, 8 mA for USB 3 are not
    /// distinguished here: this is the raw bMaxPower * 2).
    pub max_power_ma: u16,
    pub name_index: u8,
    pub interfaces: Vec<Interface>,
}

const MAX_INTERFACES: usize = 32;
const MAX_ENDPOINTS: usize = 30;

/// Reads `wTotalLength` from the first 9 bytes of a configuration
/// descriptor (so the driver can fetch the whole tree).
pub fn configuration_total_length(b: &[u8]) -> Result<u16, DescriptorError> {
    if b.len() < 9 {
        return Err(DescriptorError::Truncated);
    }
    if b[1] != DESC_CONFIGURATION {
        return Err(DescriptorError::WrongType);
    }
    if b[0] < 9 {
        return Err(DescriptorError::BadLength);
    }
    let total = u16_at(b, 2);
    if total < 9 {
        return Err(DescriptorError::BadTotalLength);
    }
    Ok(total)
}

impl Configuration {
    /// Parses a complete configuration descriptor tree (configuration,
    /// interfaces, HID class descriptors, endpoints, SuperSpeed companions;
    /// other class/vendor descriptors are skipped).
    pub fn parse(b: &[u8]) -> Result<Configuration, DescriptorError> {
        let total = configuration_total_length(b)? as usize;
        if b.len() < total {
            return Err(DescriptorError::BadTotalLength);
        }
        let b = &b[..total];
        let mut cfg = Configuration {
            value: b[5],
            attributes: b[7],
            max_power_ma: b[8] as u16 * 2,
            name_index: b[6],
            interfaces: Vec::new(),
        };
        let announced = b[4] as usize;
        // bNumEndpoints of each interface, checked against what follows.
        let mut expected: Vec<usize> = Vec::new();
        let complete = |cfg: &Configuration, expected: &[usize]| match (cfg.interfaces.last(), expected.last()) {
            (Some(i), Some(&n)) => i.endpoints.len() == n,
            _ => true,
        };
        let mut off = b[0] as usize;
        while off < b.len() {
            if b.len() - off < 2 {
                return Err(DescriptorError::BadLength);
            }
            let len = b[off] as usize;
            let kind = b[off + 1];
            if len < 2 || off + len > b.len() {
                return Err(DescriptorError::BadLength);
            }
            let d = &b[off..off + len];
            match kind {
                DESC_INTERFACE => {
                    if len < 9 {
                        return Err(DescriptorError::BadLength);
                    }
                    if cfg.interfaces.len() >= MAX_INTERFACES {
                        return Err(DescriptorError::TooManyEntries);
                    }
                    if !complete(&cfg, &expected) {
                        return Err(DescriptorError::BadStructure);
                    }
                    if d[4] as usize > MAX_ENDPOINTS {
                        return Err(DescriptorError::TooManyEntries);
                    }
                    cfg.interfaces.push(Interface {
                        number: d[2],
                        alternate: d[3],
                        class: d[5],
                        subclass: d[6],
                        protocol: d[7],
                        name_index: d[8],
                        hid: None,
                        endpoints: Vec::with_capacity(d[4] as usize),
                    });
                    expected.push(d[4] as usize);
                }
                DESC_ENDPOINT => {
                    if len < 7 {
                        return Err(DescriptorError::BadLength);
                    }
                    let want = *expected.last().ok_or(DescriptorError::BadStructure)?;
                    let iface = cfg.interfaces.last_mut().ok_or(DescriptorError::BadStructure)?;
                    if iface.endpoints.len() >= want {
                        return Err(DescriptorError::BadStructure);
                    }
                    if d[2] & 0x70 != 0 || d[2] & 0x0F == 0 {
                        // Reserved address bits set, or endpoint 0 described.
                        return Err(DescriptorError::BadStructure);
                    }
                    let mps = u16_at(d, 4);
                    iface.endpoints.push(EndpointDescriptor {
                        address: d[2],
                        attributes: d[3],
                        max_packet: mps & 0x7FF,
                        mult: ((mps >> 11) & 3) as u8,
                        interval: d[6],
                        max_burst: 0,
                        bytes_per_interval: 0,
                    });
                }
                DESC_SS_ENDPOINT_COMPANION => {
                    if len < 6 {
                        return Err(DescriptorError::BadLength);
                    }
                    let ep = cfg
                        .interfaces
                        .last_mut()
                        .and_then(|i| i.endpoints.last_mut())
                        .ok_or(DescriptorError::BadStructure)?;
                    ep.max_burst = d[2].min(15);
                    ep.bytes_per_interval = u16_at(d, 4);
                }
                DESC_HID => {
                    if len < 9 {
                        return Err(DescriptorError::BadLength);
                    }
                    let iface = cfg.interfaces.last_mut().ok_or(DescriptorError::BadStructure)?;
                    let n = d[5] as usize;
                    if n == 0 || 6 + 3 * n > len {
                        return Err(DescriptorError::BadLength);
                    }
                    let report_length = (0..n)
                        .map(|i| &d[6 + 3 * i..9 + 3 * i])
                        .find(|e| e[0] == DESC_HID_REPORT)
                        .map_or(0, |e| u16_at(e, 1));
                    iface.hid = Some(HidDescriptor { hid_version: u16_at(d, 2), country: d[4], report_length });
                }
                DESC_CONFIGURATION | DESC_DEVICE => return Err(DescriptorError::BadStructure),
                _ => {} // class- or vendor-specific: skipped
            }
            off += len;
        }
        if !complete(&cfg, &expected) {
            return Err(DescriptorError::BadStructure);
        }
        // bNumInterfaces counts interface numbers, not alternate settings.
        let mut numbers: Vec<u8> = cfg.interfaces.iter().map(|i| i.number).collect();
        numbers.sort_unstable();
        numbers.dedup();
        if numbers.len() != announced {
            return Err(DescriptorError::BadStructure);
        }
        Ok(cfg)
    }

    /// Interfaces with alternate setting 0 (the ones active after
    /// SET_CONFIGURATION).
    pub fn default_interfaces(&self) -> impl Iterator<Item = &Interface> {
        self.interfaces.iter().filter(|i| i.alternate == 0)
    }
}

/// Decodes a string descriptor (UTF-16LE) into a `String`; unpaired
/// surrogates become U+FFFD.
pub fn parse_string(b: &[u8]) -> Result<String, DescriptorError> {
    if b.len() < 2 {
        return Err(DescriptorError::Truncated);
    }
    if b[1] != DESC_STRING {
        return Err(DescriptorError::WrongType);
    }
    let len = (b[0] as usize).min(b.len());
    if len < 2 || !len.is_multiple_of(2) {
        return Err(DescriptorError::BadLength);
    }
    let units = b[2..len].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c));
    Ok(core::char::decode_utf16(units).map(|r| r.unwrap_or('\u{FFFD}')).collect())
}

/// The first language id from string descriptor 0.
pub fn first_language(b: &[u8]) -> Option<u16> {
    if b.len() < 4 || b[1] != DESC_STRING || b[0] < 4 {
        return None;
    }
    Some(u16_at(b, 2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// Configuration descriptor of QEMU's usb-kbd (hw/usb/dev-hid.c).
    const KBD_CONFIG: [u8; 34] = [
        0x09, 0x02, 0x22, 0x00, 0x01, 0x01, 0x06, 0xA0, 0x32, // configuration
        0x09, 0x04, 0x00, 0x00, 0x01, 0x03, 0x01, 0x01, 0x07, // interface 0: HID boot keyboard
        0x09, 0x21, 0x11, 0x01, 0x00, 0x01, 0x22, 0x3F, 0x00, // HID, report descriptor 63 bytes
        0x07, 0x05, 0x81, 0x03, 0x08, 0x00, 0x0A, // EP1 IN interrupt, 8 bytes, 10 ms
    ];

    const DEVICE: [u8; 18] =
        [0x12, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 0x08, 0x27, 0x06, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x0B, 0x01];

    #[test]
    fn parses_device_descriptor() {
        let d = DeviceDescriptor::parse(&DEVICE).unwrap();
        assert_eq!((d.usb_version, d.max_packet0, d.vendor, d.product), (0x0200, 8, 0x0627, 0x0001));
        assert_eq!((d.manufacturer_index, d.product_index, d.serial_index), (1, 4, 11));
        assert_eq!(d.ep0_max_packet(), 8);
        assert_eq!(DeviceDescriptor::max_packet0_from_prefix(&DEVICE[..8]), Ok(8));
        let mut usb3 = DEVICE;
        usb3[2..4].copy_from_slice(&0x0310u16.to_le_bytes());
        usb3[7] = 9;
        assert_eq!(DeviceDescriptor::parse(&usb3).unwrap().ep0_max_packet(), 512);
    }

    #[test]
    fn rejects_bad_device_descriptors() {
        assert_eq!(DeviceDescriptor::parse(&DEVICE[..7]), Err(DescriptorError::Truncated));
        assert_eq!(DeviceDescriptor::parse(&DEVICE[..12]), Err(DescriptorError::Truncated));
        let mut d = DEVICE;
        d[1] = 2;
        assert_eq!(DeviceDescriptor::parse(&d), Err(DescriptorError::WrongType));
        let mut d = DEVICE;
        d[0] = 10;
        assert_eq!(DeviceDescriptor::parse(&d), Err(DescriptorError::BadLength));
    }

    #[test]
    fn parses_hid_keyboard_configuration() {
        assert_eq!(configuration_total_length(&KBD_CONFIG[..9]), Ok(34));
        let c = Configuration::parse(&KBD_CONFIG).unwrap();
        assert_eq!((c.value, c.max_power_ma), (1, 100));
        assert_eq!(c.interfaces.len(), 1);
        let i = &c.interfaces[0];
        assert!(i.is_hid_boot(HID_PROTOCOL_KEYBOARD));
        assert!(!i.is_hid_boot(HID_PROTOCOL_MOUSE));
        assert_eq!(i.hid.unwrap().report_length, 63);
        let ep = i.interrupt_in().unwrap();
        assert_eq!((ep.address, ep.max_packet, ep.interval, ep.number()), (0x81, 8, 10, 1));
        assert_eq!(c.default_interfaces().count(), 1);
    }

    #[test]
    fn skips_unknown_descriptors_and_reads_companions() {
        let mut v = KBD_CONFIG.to_vec();
        // A vendor descriptor between HID and endpoint, and a companion after it.
        v.splice(27..27, [0x04, 0xFF, 0xAA, 0xBB]);
        v.extend_from_slice(&[0x06, 0x30, 0x03, 0x00, 0x08, 0x00]);
        let total = v.len() as u16;
        v[2..4].copy_from_slice(&total.to_le_bytes());
        let c = Configuration::parse(&v).unwrap();
        let ep = c.interfaces[0].endpoints[0];
        assert_eq!((ep.max_burst, ep.bytes_per_interval), (3, 8));
    }

    #[test]
    fn rejects_malformed_configuration_trees() {
        // wTotalLength larger than the data.
        assert_eq!(Configuration::parse(&KBD_CONFIG[..30]), Err(DescriptorError::BadTotalLength));
        // A descriptor whose bLength runs past the end.
        let mut v = KBD_CONFIG;
        v[27] = 0x09;
        assert_eq!(Configuration::parse(&v), Err(DescriptorError::BadLength));
        // bLength 0 would loop forever if not caught.
        let mut v = KBD_CONFIG;
        v[9] = 0;
        assert_eq!(Configuration::parse(&v), Err(DescriptorError::BadLength));
        // Interface announces 2 endpoints but has 1.
        let mut v = KBD_CONFIG;
        v[13] = 2;
        assert_eq!(Configuration::parse(&v), Err(DescriptorError::BadStructure));
        // Endpoint before any interface.
        let mut v = vec![0x09, 0x02, 16, 0, 0, 1, 0, 0x80, 0];
        v.extend_from_slice(&[0x07, 0x05, 0x81, 0x03, 0x08, 0x00, 0x0A]);
        assert_eq!(Configuration::parse(&v), Err(DescriptorError::BadStructure));
        // Endpoint 0 described.
        let mut v = KBD_CONFIG;
        v[29] = 0x80;
        assert_eq!(Configuration::parse(&v), Err(DescriptorError::BadStructure));
        // bNumInterfaces wrong.
        let mut v = KBD_CONFIG;
        v[4] = 2;
        assert_eq!(Configuration::parse(&v), Err(DescriptorError::BadStructure));
        // HID descriptor claiming more class descriptors than it holds.
        let mut v = KBD_CONFIG;
        v[23] = 5;
        assert_eq!(Configuration::parse(&v), Err(DescriptorError::BadLength));
        // Wrong type / short header.
        assert_eq!(configuration_total_length(&DEVICE), Err(DescriptorError::WrongType));
        assert_eq!(configuration_total_length(&KBD_CONFIG[..5]), Err(DescriptorError::Truncated));
    }

    #[test]
    fn malformed_trees_never_panic() {
        // Every single-byte corruption of a valid tree parses or errors.
        for i in 0..KBD_CONFIG.len() {
            for v in [0u8, 1, 2, 7, 9, 0x21, 0x7F, 0xFF] {
                let mut b = KBD_CONFIG;
                b[i] = v;
                let _ = Configuration::parse(&b);
                for cut in 0..b.len() {
                    let _ = Configuration::parse(&b[..cut]);
                }
            }
        }
    }

    #[test]
    fn decodes_strings() {
        let s = [0x0C, 0x03, b'Q', 0, b'E', 0, b'M', 0, b'U', 0, 0x7C, 0x01];
        assert_eq!(parse_string(&s).unwrap(), "QEMU\u{17C}");
        assert_eq!(first_language(&[4, 3, 0x09, 0x04]), Some(0x0409));
        assert_eq!(first_language(&[2, 3]), None);
        assert_eq!(parse_string(&[3, 3, 0]), Err(DescriptorError::BadLength));
        assert_eq!(parse_string(&[4, 2, 0, 0]), Err(DescriptorError::WrongType));
        // Unpaired surrogate.
        assert_eq!(parse_string(&[4, 3, 0x00, 0xD8]).unwrap(), "\u{FFFD}");
        // bLength longer than the data: truncated to what is there.
        assert_eq!(parse_string(&[0x20, 3, b'A', 0]).unwrap(), "A");
    }

    #[test]
    fn setup_packets() {
        let p = SetupPacket::get_interface_descriptor(DESC_HID_REPORT, 0, 63);
        assert_eq!(p.to_u64(), 0x003F_0000_2200_0681);
        assert!(p.is_in());
        assert_eq!(SetupPacket::hid_set_protocol(1, false).to_u64(), 0x0000_0001_0000_0B21);
        assert_eq!(SetupPacket::hid_set_idle(0, 0).to_u64(), 0x0A21);
        assert_eq!(SetupPacket::hid_set_output_report(0, 0, 1).to_u64(), 0x0001_0000_0200_0921);
        assert!(!SetupPacket::set_configuration(1).is_in());
    }
}
