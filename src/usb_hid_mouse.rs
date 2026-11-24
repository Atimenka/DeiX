//! Parsing of USB HID boot-protocol mouse reports.
//!
//! This is only a report parser, not a USB host controller, enumeration,
//! hub, HID setup, or polling driver. A report is valid here only after a
//! host has selected a boot-mouse interface and requested boot protocol.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootMouseReport {
    pub buttons: u8,
    pub dx: i8,
    pub dy: i8,
}

/// Boot protocol reports have three mandatory bytes. Some mice append a
/// wheel byte; ignore it because the existing UI has no wheel event path.
pub fn parse_boot_mouse_report(bytes: &[u8]) -> Option<BootMouseReport> {
    if bytes.len() < 3 { return None; }
    Some(BootMouseReport {
        buttons: bytes[0] & 0x07,
        dx: bytes[1] as i8,
        dy: bytes[2] as i8,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_truncated_reports() {
        assert_eq!(parse_boot_mouse_report(&[]), None);
        assert_eq!(parse_boot_mouse_report(&[1]), None);
        assert_eq!(parse_boot_mouse_report(&[1, 2]), None);
    }
    #[test]
    fn parses_signed_relative_movement_and_buttons() {
        assert_eq!(parse_boot_mouse_report(&[0xff, 0x80, 0x7f]),
            Some(BootMouseReport { buttons: 7, dx: -128, dy: 127 }));
        assert_eq!(parse_boot_mouse_report(&[1, 0xff, 0x01, 0x55]),
            Some(BootMouseReport { buttons: 1, dx: -1, dy: 1 }));
        assert_eq!(parse_boot_mouse_report(&[0, 0, 0]),
            Some(BootMouseReport { buttons: 0, dx: 0, dy: 0 }));
    }
}
