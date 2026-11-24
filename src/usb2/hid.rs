//! HID report decoding: boot protocol keyboards and mice, and a report
//! descriptor parser for pointing devices (HID 1.11 chapters 6 and 8,
//! appendix B).
//!
//! Key codes are HID Keyboard/Keypad usage ids (usage page 0x07); they are
//! the raw key identity OSyx passes on. Turning them into characters is the
//! job of a keyboard layout, outside this crate.

use alloc::vec::Vec;

/// Usage ids of the modifier keys (left control .. right GUI), in the bit
/// order of the boot keyboard modifier byte.
pub const MODIFIER_USAGES: [u8; 8] = [0xE0, 0xE1, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7];
/// "ErrorRollOver": the keyboard reports too many keys at once.
pub const USAGE_ERROR_ROLLOVER: u8 = 0x01;

/// A key went down or up (HID usage id on page 0x07).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyChange {
    pub usage: u8,
    pub pressed: bool,
}

/// Turns successive 8-byte boot keyboard reports into key changes.
#[derive(Clone, Debug, Default)]
pub struct BootKeyboard {
    modifiers: u8,
    keys: [u8; 6],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportError {
    /// Shorter than the report format requires.
    Truncated,
    /// The keyboard signalled a rollover error (more keys than it can
    /// report): the previous state is kept, nothing is generated.
    Rollover,
    /// A report id that the descriptor does not describe.
    UnknownReportId,
}

impl BootKeyboard {
    pub fn new() -> BootKeyboard {
        BootKeyboard::default()
    }

    /// Currently held modifier bits (boot report byte 0).
    pub fn modifiers(&self) -> u8 {
        self.modifiers
    }

    /// Decodes one report and appends the changes since the previous one:
    /// releases first, then presses, modifiers before ordinary keys.
    pub fn update(&mut self, report: &[u8], out: &mut Vec<KeyChange>) -> Result<(), ReportError> {
        if report.len() < 3 {
            return Err(ReportError::Truncated);
        }
        let mods = report[0];
        let mut keys = [0u8; 6];
        for (i, &k) in report[2..report.len().min(8)].iter().enumerate() {
            keys[i] = k;
        }
        if keys.iter().any(|&k| (USAGE_ERROR_ROLLOVER..=0x03).contains(&k)) {
            return Err(ReportError::Rollover);
        }
        // Releases.
        for (bit, &usage) in MODIFIER_USAGES.iter().enumerate() {
            if self.modifiers & (1 << bit) != 0 && mods & (1 << bit) == 0 {
                out.push(KeyChange { usage, pressed: false });
            }
        }
        for &k in self.keys.iter().filter(|&&k| k != 0) {
            if !keys.contains(&k) {
                out.push(KeyChange { usage: k, pressed: false });
            }
        }
        // Presses.
        for (bit, &usage) in MODIFIER_USAGES.iter().enumerate() {
            if self.modifiers & (1 << bit) == 0 && mods & (1 << bit) != 0 {
                out.push(KeyChange { usage, pressed: true });
            }
        }
        for (i, &k) in keys.iter().enumerate() {
            // Report each newly pressed key once even if a buggy device
            // repeats it within the array.
            if k != 0 && !self.keys.contains(&k) && !keys[..i].contains(&k) {
                out.push(KeyChange { usage: k, pressed: true });
            }
        }
        self.modifiers = mods;
        self.keys = keys;
        Ok(())
    }

    /// Releases everything held (device unplugged): the changes needed so
    /// that no key stays stuck.
    pub fn release_all(&mut self, out: &mut Vec<KeyChange>) {
        let _ = self.update(&[0; 8], out);
    }
}

/// Keyboard LED output report bits.
pub mod led {
    pub const NUM_LOCK: u8 = 1 << 0;
    pub const CAPS_LOCK: u8 = 1 << 1;
    pub const SCROLL_LOCK: u8 = 1 << 2;
}

/// One decoded mouse report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseReport {
    /// Bit `n` = button `n + 1` (left, right, middle, back, forward, ...).
    pub buttons: u32,
    pub dx: i32,
    pub dy: i32,
    pub wheel: i32,
    pub pan: i32,
}

/// Decodes a boot protocol mouse report (buttons, X, Y and, if the device
/// sends a fourth byte, a wheel; HID 1.11 appendix B.2).
pub fn boot_mouse(report: &[u8]) -> Result<MouseReport, ReportError> {
    if report.len() < 3 {
        return Err(ReportError::Truncated);
    }
    Ok(MouseReport {
        buttons: (report[0] & 0x1F) as u32,
        dx: report[1] as i8 as i32,
        dy: report[2] as i8 as i32,
        wheel: report.get(3).map_or(0, |&w| w as i8 as i32),
        pan: 0,
    })
}

// ---- report descriptors ----------------------------------------------------

pub const PAGE_GENERIC_DESKTOP: u16 = 0x01;
pub const PAGE_KEYBOARD: u16 = 0x07;
pub const PAGE_BUTTON: u16 = 0x09;
pub const PAGE_CONSUMER: u16 = 0x0C;
pub const USAGE_MOUSE: u16 = 0x02;
pub const USAGE_X: u16 = 0x30;
pub const USAGE_Y: u16 = 0x31;
pub const USAGE_WHEEL: u16 = 0x38;
pub const USAGE_AC_PAN: u16 = 0x238;
pub const PAGE_DIGITIZER: u16 = 0x0D;
pub const USAGE_TIP_SWITCH: u16 = 0x42;

/// One input field of a report: `size` bits at `offset` (bits from the
/// start of the report data, after the report id byte if there is one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    pub report_id: u8,
    pub offset: u32,
    pub size: u32,
    pub page: u16,
    pub usage: u16,
    pub logical_min: i32,
    pub logical_max: i32,
    /// Physical range and unit (HID items Physical Minimum/Maximum, Unit,
    /// Unit Exponent); all zero when the descriptor does not give them.
    pub physical_min: i32,
    pub physical_max: i32,
    pub unit: u32,
    pub unit_exponent: i8,
    pub relative: bool,
}

impl Field {
    /// Extracts the field from `data` (the report without its id byte),
    /// sign-extending when the logical range is signed.
    pub fn read(&self, data: &[u8]) -> Option<i32> {
        if self.size == 0 || self.size > 32 {
            return None;
        }
        let end = self.offset.checked_add(self.size)?;
        if end as usize > data.len() * 8 {
            return None;
        }
        let mut v: u64 = 0;
        for i in 0..self.size {
            let bit = self.offset + i;
            if data[(bit / 8) as usize] & (1 << (bit % 8)) != 0 {
                v |= 1 << i;
            }
        }
        if self.logical_min < 0 && self.size < 32 && v & (1 << (self.size - 1)) != 0 {
            v |= !0u64 << self.size;
        }
        Some(v as i64 as i32)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// An item runs past the end of the descriptor.
    Truncated,
    /// Collections nested deeper than the parser allows, or an End
    /// Collection without a Collection.
    BadNesting,
    /// Push/Pop imbalance.
    BadStack,
    /// Report size/count out of range or too many fields.
    TooLarge,
}

#[derive(Clone, Copy, Debug, Default)]
struct Globals {
    page: u16,
    logical_min: i32,
    logical_max: i32,
    physical_min: i32,
    physical_max: i32,
    unit: u32,
    unit_exponent: i8,
    report_size: u32,
    report_count: u32,
    report_id: u8,
}

const MAX_FIELDS: usize = 512;
const MAX_DEPTH: usize = 16;
const MAX_STACK: usize = 8;
const MAX_USAGES: usize = 256;

/// All variable input fields described by a report descriptor (array
/// fields such as keyboard key arrays are recorded with usage 0).
pub fn parse_report_descriptor(desc: &[u8]) -> Result<Vec<Field>, ParseError> {
    let mut fields = Vec::new();
    let mut g = Globals::default();
    let mut stack: Vec<Globals> = Vec::new();
    // (page, usage) list of the current main item; a usage with an explicit
    // page (4-byte usage) keeps it.
    let mut usages: Vec<(u16, u16)> = Vec::new();
    let mut usage_min: Option<u32> = None;
    let mut usage_max: Option<u32> = None;
    let mut depth = 0usize;
    // Bit offsets per report id.
    let mut offsets: Vec<(u8, u32)> = Vec::new();
    let mut i = 0usize;
    while i < desc.len() {
        let prefix = desc[i];
        if prefix == 0xFE {
            // Long item: bDataSize, bLongItemTag, data.
            let size = *desc.get(i + 1).ok_or(ParseError::Truncated)? as usize;
            i = i.checked_add(3 + size).ok_or(ParseError::Truncated)?;
            if i > desc.len() {
                return Err(ParseError::Truncated);
            }
            continue;
        }
        let size = match prefix & 3 {
            3 => 4,
            n => n as usize,
        };
        let data = desc.get(i + 1..i + 1 + size).ok_or(ParseError::Truncated)?;
        let mut uval: u32 = 0;
        for (k, &b) in data.iter().enumerate() {
            uval |= (b as u32) << (8 * k);
        }
        let sval: i32 = match size {
            1 => data[0] as i8 as i32,
            2 => u16::from_le_bytes([data[0], data[1]]) as i16 as i32,
            4 => uval as i32,
            _ => 0,
        };
        let kind = (prefix >> 2) & 3;
        let tag = prefix >> 4;
        match (kind, tag) {
            // ---- main items
            (0, 0x8) => {
                // Input
                let constant = uval & 1 != 0;
                let variable = uval & 2 != 0;
                let relative = uval & 4 != 0;
                if g.report_size > 32 || g.report_count > 1024 {
                    return Err(ParseError::TooLarge);
                }
                let pos = match offsets.iter_mut().find(|(id, _)| *id == g.report_id) {
                    Some((_, off)) => off,
                    None => {
                        offsets.push((g.report_id, 0));
                        &mut offsets.last_mut().expect("just pushed").1
                    }
                };
                let mut range_usages = Vec::new();
                if let (Some(lo), Some(hi)) = (usage_min, usage_max) {
                    if hi >= lo && hi - lo < MAX_USAGES as u32 {
                        for u in lo..=hi {
                            let page = if u > 0xFFFF { (u >> 16) as u16 } else { g.page };
                            range_usages.push((page, u as u16));
                        }
                    }
                }
                let all: Vec<(u16, u16)> = usages.iter().copied().chain(range_usages).collect();
                for n in 0..g.report_count {
                    if !constant {
                        let (page, usage) = if variable {
                            all.get(n as usize).or(all.last()).copied().unwrap_or((g.page, 0))
                        } else {
                            (g.page, 0)
                        };
                        if fields.len() >= MAX_FIELDS {
                            return Err(ParseError::TooLarge);
                        }
                        fields.push(Field {
                            report_id: g.report_id,
                            offset: *pos,
                            size: g.report_size,
                            page,
                            usage,
                            logical_min: g.logical_min,
                            logical_max: g.logical_max,
                            physical_min: g.physical_min,
                            physical_max: g.physical_max,
                            unit: g.unit,
                            unit_exponent: g.unit_exponent,
                            relative,
                        });
                    }
                    *pos = pos.checked_add(g.report_size).ok_or(ParseError::TooLarge)?;
                }
                usages.clear();
                usage_min = None;
                usage_max = None;
            }
            (0, 0x9) | (0, 0xB) => {
                // Output / Feature: not input data, but they end the local state.
                usages.clear();
                usage_min = None;
                usage_max = None;
            }
            (0, 0xA) => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err(ParseError::BadNesting);
                }
                usages.clear();
                usage_min = None;
                usage_max = None;
            }
            (0, 0xC) => {
                depth = depth.checked_sub(1).ok_or(ParseError::BadNesting)?;
            }
            // ---- global items
            (1, 0x0) => g.page = uval as u16,
            (1, 0x1) => g.logical_min = sval,
            (1, 0x2) => {
                // Logical Maximum is unsigned when the minimum is not negative.
                g.logical_max = if g.logical_min >= 0 { uval as i32 } else { sval };
            }
            (1, 0x3) => g.physical_min = sval,
            (1, 0x4) => g.physical_max = if g.physical_min >= 0 { uval as i32 } else { sval },
            (1, 0x5) => {
                // Unit Exponent: a signed nibble.
                let n = (uval & 0xF) as i8;
                g.unit_exponent = if n > 7 { n - 16 } else { n };
            }
            (1, 0x6) => g.unit = uval,
            (1, 0x7) => g.report_size = uval,
            (1, 0x8) => {
                if uval == 0 || uval > 255 {
                    return Err(ParseError::TooLarge);
                }
                g.report_id = uval as u8;
            }
            (1, 0x9) => g.report_count = uval,
            (1, 0xA) => {
                if stack.len() >= MAX_STACK {
                    return Err(ParseError::BadStack);
                }
                stack.push(g);
            }
            (1, 0xB) => g = stack.pop().ok_or(ParseError::BadStack)?,
            // ---- local items
            (2, 0x0) => {
                if usages.len() < MAX_USAGES {
                    let page = if size == 4 { (uval >> 16) as u16 } else { g.page };
                    usages.push((page, uval as u16));
                }
            }
            (2, 0x1) => usage_min = Some(if size == 4 { uval } else { uval & 0xFFFF }),
            (2, 0x2) => usage_max = Some(if size == 4 { uval } else { uval & 0xFFFF }),
            _ => {} // physical ranges, units, designators, strings, delimiters
        }
        i += 1 + size;
    }
    if depth != 0 {
        return Err(ParseError::BadNesting);
    }
    Ok(fields)
}

/// Where a pointing device puts its data, from its report descriptor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MouseLayout {
    /// The report carrying X/Y (0 = the device uses no report ids).
    pub report_id: u8,
    /// (button number starting at 1, field)
    pub buttons: Vec<(u8, Field)>,
    pub x: Field,
    pub y: Field,
    pub wheel: Option<Field>,
    pub pan: Option<Field>,
}

impl MouseLayout {
    /// Finds relative X and Y (Generic Desktop) and their companions in the
    /// same report. `None` if the device has no relative X/Y (an absolute
    /// tablet or not a pointer at all).
    pub fn from_fields(fields: &[Field]) -> Option<MouseLayout> {
        let find = |page: u16, usage: u16| fields.iter().find(|f| f.page == page && f.usage == usage && f.relative).copied();
        let x = find(PAGE_GENERIC_DESKTOP, USAGE_X)?;
        let y = fields
            .iter()
            .find(|f| f.page == PAGE_GENERIC_DESKTOP && f.usage == USAGE_Y && f.relative && f.report_id == x.report_id)
            .copied()?;
        let same = |f: &&Field| f.report_id == x.report_id;
        let buttons = fields
            .iter()
            .filter(same)
            .filter(|f| f.page == PAGE_BUTTON && (1..=32).contains(&f.usage) && f.size >= 1)
            .map(|f| (f.usage as u8, *f))
            .collect();
        let wheel = fields.iter().filter(same).find(|f| f.page == PAGE_GENERIC_DESKTOP && f.usage == USAGE_WHEEL && f.relative).copied();
        let pan = fields.iter().filter(same).find(|f| f.page == PAGE_CONSUMER && f.usage == USAGE_AC_PAN && f.relative).copied();
        Some(MouseLayout { report_id: x.report_id, buttons, x, y, wheel, pan })
    }

    /// Decodes one interrupt IN report. With report ids, the first byte is
    /// the id; reports for other ids are rejected.
    pub fn decode(&self, report: &[u8]) -> Result<MouseReport, ReportError> {
        let data = if self.report_id != 0 {
            match report.split_first() {
                Some((&id, rest)) if id == self.report_id => rest,
                Some(_) => return Err(ReportError::UnknownReportId),
                None => return Err(ReportError::Truncated),
            }
        } else {
            report
        };
        self.decode_data(data)
    }

    /// Decodes the data of a report whose id byte (if any) is already
    /// removed and checked.
    pub fn decode_data(&self, data: &[u8]) -> Result<MouseReport, ReportError> {
        let x = self.x.read(data).ok_or(ReportError::Truncated)?;
        let y = self.y.read(data).ok_or(ReportError::Truncated)?;
        let mut buttons = 0;
        for (n, f) in &self.buttons {
            if f.read(data).is_some_and(|v| v != 0) {
                buttons |= 1 << (n - 1);
            }
        }
        Ok(MouseReport {
            buttons,
            dx: x,
            dy: y,
            wheel: self.wheel.and_then(|f| f.read(data)).unwrap_or(0),
            pan: self.pan.and_then(|f| f.read(data)).unwrap_or(0),
        })
    }

    /// Bytes of a report (including the id byte).
    pub fn report_len(&self) -> usize {
        let bits = [Some(self.x), Some(self.y), self.wheel, self.pan]
            .into_iter()
            .flatten()
            .chain(self.buttons.iter().map(|(_, f)| *f))
            .map(|f| f.offset + f.size)
            .max()
            .unwrap_or(0);
        bits.div_ceil(8) as usize + usize::from(self.report_id != 0)
    }
}

/// One decoded report of an absolute pointing device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TabletReport {
    /// Bit `n` = button `n + 1`; a pen's or finger's tip counts as button 1.
    pub buttons: u32,
    /// 0 (left, top) to [`TabletLayout::RANGE`] (right, bottom).
    pub x: i32,
    pub y: i32,
    pub wheel: i32,
}

/// Where an absolute pointing device (a tablet, a touch screen, the
/// pointer QEMU, VirtualBox and remote consoles present so the guest's
/// pointer follows the host's) keeps position, buttons and wheel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabletLayout {
    pub report_id: u8,
    /// (button number starting at 1, field)
    pub buttons: Vec<(u8, Field)>,
    pub x: Field,
    pub y: Field,
    pub wheel: Option<Field>,
}

impl TabletLayout {
    /// Positions are scaled to 0..=RANGE whatever the device's own range.
    pub const RANGE: i32 = 0xFFFF;

    /// Finds absolute X and Y (Generic Desktop) and their companions in
    /// the same report. `None` if the device has no absolute X/Y with a
    /// usable range.
    pub fn from_fields(fields: &[Field]) -> Option<TabletLayout> {
        let axis = |usage: u16, id: Option<u8>| {
            fields
                .iter()
                .find(|f| {
                    f.page == PAGE_GENERIC_DESKTOP
                        && f.usage == usage
                        && !f.relative
                        && f.logical_max > f.logical_min
                        && id.is_none_or(|id| f.report_id == id)
                })
                .copied()
        };
        let x = axis(USAGE_X, None)?;
        let y = axis(USAGE_Y, Some(x.report_id))?;
        let same = |f: &&Field| f.report_id == x.report_id;
        let mut buttons: Vec<(u8, Field)> = fields
            .iter()
            .filter(same)
            .filter(|f| f.page == PAGE_BUTTON && (1..=32).contains(&f.usage) && f.size >= 1)
            .map(|f| (f.usage as u8, *f))
            .collect();
        // A touch screen or pen has no buttons: its tip is the left one.
        if !buttons.iter().any(|(n, _)| *n == 1) {
            if let Some(tip) = fields.iter().filter(same).find(|f| f.page == PAGE_DIGITIZER && f.usage == USAGE_TIP_SWITCH) {
                buttons.push((1, *tip));
            }
        }
        let wheel = fields.iter().filter(same).find(|f| f.page == PAGE_GENERIC_DESKTOP && f.usage == USAGE_WHEEL && f.relative).copied();
        Some(TabletLayout { report_id: x.report_id, buttons, x, y, wheel })
    }

    fn scale(f: &Field, v: i32) -> i32 {
        let span = f.logical_max as i64 - f.logical_min as i64;
        let v = (v as i64).clamp(f.logical_min as i64, f.logical_max as i64) - f.logical_min as i64;
        (v * Self::RANGE as i64 / span) as i32
    }

    /// Decodes one interrupt IN report. With report ids, the first byte is
    /// the id; reports for other ids are rejected.
    pub fn decode(&self, report: &[u8]) -> Result<TabletReport, ReportError> {
        let data = if self.report_id != 0 {
            match report.split_first() {
                Some((&id, rest)) if id == self.report_id => rest,
                Some(_) => return Err(ReportError::UnknownReportId),
                None => return Err(ReportError::Truncated),
            }
        } else {
            report
        };
        let x = self.x.read(data).ok_or(ReportError::Truncated)?;
        let y = self.y.read(data).ok_or(ReportError::Truncated)?;
        let mut buttons = 0;
        for (n, f) in &self.buttons {
            if f.read(data).is_some_and(|v| v != 0) {
                buttons |= 1 << (n - 1);
            }
        }
        Ok(TabletReport {
            buttons,
            x: Self::scale(&self.x, x),
            y: Self::scale(&self.y, y),
            wheel: self.wheel.and_then(|f| f.read(data)).unwrap_or(0),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changes(kb: &mut BootKeyboard, report: [u8; 8]) -> Vec<KeyChange> {
        let mut v = Vec::new();
        kb.update(&report, &mut v).unwrap();
        v
    }

    fn down(usage: u8) -> KeyChange {
        KeyChange { usage, pressed: true }
    }

    fn up(usage: u8) -> KeyChange {
        KeyChange { usage, pressed: false }
    }

    #[test]
    fn keyboard_press_release_and_modifiers() {
        let mut kb = BootKeyboard::new();
        // Shift + A
        assert_eq!(changes(&mut kb, [0x02, 0, 0x04, 0, 0, 0, 0, 0]), [down(0xE1), down(0x04)]);
        // Add B, keep A: only B is new.
        assert_eq!(changes(&mut kb, [0x02, 0, 0x04, 0x05, 0, 0, 0, 0]), [down(0x05)]);
        // Same report again: nothing.
        assert!(changes(&mut kb, [0x02, 0, 0x04, 0x05, 0, 0, 0, 0]).is_empty());
        // Release shift and A; B moves to slot 0.
        assert_eq!(changes(&mut kb, [0x00, 0, 0x05, 0, 0, 0, 0, 0]), [up(0xE1), up(0x04)]);
        assert_eq!(kb.modifiers(), 0);
        // Right Alt (AltGr) + left ctrl.
        assert_eq!(changes(&mut kb, [0x41, 0, 0x05, 0, 0, 0, 0, 0]), [down(0xE0), down(0xE6)]);
        let mut all = Vec::new();
        kb.release_all(&mut all);
        assert_eq!(all, [up(0xE0), up(0xE6), up(0x05)]);
    }

    #[test]
    fn keyboard_rollover_and_malformed_reports() {
        let mut kb = BootKeyboard::new();
        changes(&mut kb, [0, 0, 0x04, 0, 0, 0, 0, 0]);
        let mut v = Vec::new();
        assert_eq!(kb.update(&[0, 0, 1, 1, 1, 1, 1, 1], &mut v), Err(ReportError::Rollover));
        assert!(v.is_empty(), "a rollover report changes nothing");
        assert_eq!(kb.update(&[0, 0], &mut v), Err(ReportError::Truncated));
        // Six-key rollover: all six reported, duplicates once.
        let mut kb = BootKeyboard::new();
        let got = changes(&mut kb, [0, 0, 4, 5, 6, 7, 8, 8]);
        assert_eq!(got, [down(4), down(5), down(6), down(7), down(8)]);
        // A short report (some devices send fewer key slots).
        let mut kb = BootKeyboard::new();
        kb.update(&[0, 0, 0x2C], &mut v).unwrap();
        assert_eq!(v, [down(0x2C)]);
    }

    #[test]
    fn boot_mouse_reports() {
        assert_eq!(boot_mouse(&[1, 5, 0xFB]).unwrap(), MouseReport { buttons: 1, dx: 5, dy: -5, wheel: 0, pan: 0 });
        assert_eq!(boot_mouse(&[6, 0x80, 0x7F, 0xFF]).unwrap(), MouseReport { buttons: 6, dx: -128, dy: 127, wheel: -1, pan: 0 });
        assert_eq!(boot_mouse(&[0, 0]), Err(ReportError::Truncated));
    }

    /// Report descriptor of QEMU's usb-mouse (hw/usb/dev-hid.c).
    const QEMU_MOUSE: [u8; 52] = [
        0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x09, 0x01, 0xA1, 0x00, 0x05, 0x09, 0x19, 0x01, 0x29, 0x03, 0x15, 0x00,
        0x25, 0x01, 0x95, 0x03, 0x75, 0x01, 0x81, 0x02, 0x95, 0x01, 0x75, 0x05, 0x81, 0x01, 0x05, 0x01, 0x09, 0x30,
        0x09, 0x31, 0x09, 0x38, 0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x03, 0x81, 0x06, 0xC0, 0xC0,
    ];

    #[test]
    fn parses_qemu_mouse_descriptor() {
        let fields = parse_report_descriptor(&QEMU_MOUSE).unwrap();
        let layout = MouseLayout::from_fields(&fields).unwrap();
        assert_eq!(layout.report_id, 0);
        assert_eq!(layout.buttons.len(), 3);
        assert_eq!((layout.x.offset, layout.x.size), (8, 8));
        assert_eq!((layout.y.offset, layout.wheel.unwrap().offset), (16, 24));
        assert_eq!(layout.report_len(), 4);
        let r = layout.decode(&[0x05, 0x03, 0xFE, 0x01]).unwrap();
        assert_eq!(r, MouseReport { buttons: 0b101, dx: 3, dy: -2, wheel: 1, pan: 0 });
        assert_eq!(layout.decode(&[0x01, 0x10]), Err(ReportError::Truncated));
    }

    #[test]
    fn report_ids_twelve_bit_axes_and_pan() {
        // Report id 2; 5 buttons; X/Y 12-bit signed; wheel 8-bit; AC Pan (4-byte usage).
        let desc = [
            0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x85, 0x02, 0x05, 0x09, 0x19, 0x01, 0x29, 0x05, 0x15, 0x00, 0x25, 0x01,
            0x75, 0x01, 0x95, 0x05, 0x81, 0x02, 0x75, 0x03, 0x95, 0x01, 0x81, 0x03, 0x05, 0x01, 0x09, 0x30, 0x09, 0x31,
            0x16, 0x01, 0xF8, 0x26, 0xFF, 0x07, 0x75, 0x0C, 0x95, 0x02, 0x81, 0x06, 0x09, 0x38, 0x15, 0x81, 0x25, 0x7F,
            0x75, 0x08, 0x95, 0x01, 0x81, 0x06, 0x0B, 0x38, 0x02, 0x0C, 0x00, 0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95,
            0x01, 0x81, 0x06, 0xC0,
        ];
        let fields = parse_report_descriptor(&desc).unwrap();
        let l = MouseLayout::from_fields(&fields).unwrap();
        assert_eq!(l.report_id, 2);
        assert_eq!(l.buttons.len(), 5);
        assert_eq!((l.x.offset, l.x.size, l.y.offset), (8, 12, 20));
        assert_eq!(l.wheel.unwrap().offset, 32);
        assert_eq!(l.pan.unwrap().offset, 40);
        assert_eq!(l.report_len(), 7);
        // id 2, buttons 0b10010, X = -2 (0xFFE), Y = 0x123, wheel -3, pan 2.
        let x: u32 = 0xFFE;
        let y: u32 = 0x123;
        let xy = x | (y << 12);
        let report = [2, 0b10010, xy as u8, (xy >> 8) as u8, (xy >> 16) as u8, 0xFD, 0x02];
        let r = l.decode(&report).unwrap();
        assert_eq!(r, MouseReport { buttons: 0b10010, dx: -2, dy: 0x123, wheel: -3, pan: 2 });
        assert_eq!(l.decode(&[3, 0, 0, 0, 0, 0, 0]), Err(ReportError::UnknownReportId));
        assert_eq!(l.decode(&[]), Err(ReportError::Truncated));
    }

    #[test]
    fn absolute_tablets_are_not_mice() {
        // Like QEMU's usb-tablet: absolute X/Y.
        let desc = [
            0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x15, 0x00, 0x26, 0xFF, 0x7F, 0x75,
            0x10, 0x95, 0x02, 0x81, 0x02, 0xC0,
        ];
        let fields = parse_report_descriptor(&desc).unwrap();
        assert_eq!(fields.len(), 2);
        assert!(!fields[0].relative);
        assert_eq!(fields[0].logical_max, 0x7FFF);
        assert!(MouseLayout::from_fields(&fields).is_none());
        assert!(TabletLayout::from_fields(&fields).is_some());
    }

    /// QEMU's `usb-tablet`: 3 buttons, 15-bit absolute X/Y, a wheel.
    const QEMU_TABLET: [u8; 74] = [
        0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x09, 0x01, 0xA1, 0x00, 0x05, 0x09, 0x19, 0x01, 0x29, 0x03, 0x15, 0x00,
        0x25, 0x01, 0x95, 0x03, 0x75, 0x01, 0x81, 0x02, 0x95, 0x01, 0x75, 0x05, 0x81, 0x01, 0x05, 0x01, 0x09, 0x30,
        0x09, 0x31, 0x15, 0x00, 0x26, 0xFF, 0x7F, 0x35, 0x00, 0x46, 0xFF, 0x7F, 0x75, 0x10, 0x95, 0x02, 0x81, 0x02,
        0x05, 0x01, 0x09, 0x38, 0x15, 0x81, 0x25, 0x7F, 0x35, 0x00, 0x45, 0x00, 0x75, 0x08, 0x95, 0x01, 0x81, 0x06,
        0xC0, 0xC0,
    ];

    #[test]
    fn absolute_tablet_positions_are_scaled_to_the_full_range() {
        let fields = parse_report_descriptor(&QEMU_TABLET).unwrap();
        assert!(MouseLayout::from_fields(&fields).is_none(), "not a relative pointer");
        let l = TabletLayout::from_fields(&fields).unwrap();
        assert_eq!((l.report_id, l.buttons.len()), (0, 3));
        assert_eq!((l.x.offset, l.x.size, l.y.offset, l.wheel.unwrap().offset), (8, 16, 24, 40));
        // Left button, the far right, the middle row, wheel up one.
        let r = l.decode(&[0x01, 0xFF, 0x7F, 0x00, 0x40, 0x01]).unwrap();
        assert_eq!(r, TabletReport { buttons: 1, x: 0xFFFF, y: 0x4000 * 0xFFFF / 0x7FFF, wheel: 1 });
        let r = l.decode(&[0x00, 0x00, 0x00, 0x00, 0x00, 0xFF]).unwrap();
        assert_eq!(r, TabletReport { buttons: 0, x: 0, y: 0, wheel: -1 });
        assert_eq!(l.decode(&[0x00, 0x00]), Err(ReportError::Truncated));
        // A relative mouse is not a tablet.
        assert!(TabletLayout::from_fields(&parse_report_descriptor(&QEMU_MOUSE).unwrap()).is_none());
    }

    #[test]
    fn touch_screen_tip_is_the_left_button() {
        // Digitizer / Touch Screen, report id 1: Tip Switch (1 bit), 7 bits
        // of padding, X 0..4095 and Y 0..2047, 16 bits each.
        let desc = [
            0x05, 0x0D, 0x09, 0x04, 0xA1, 0x01, 0x85, 0x01, 0x09, 0x42, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x01,
            0x81, 0x02, 0x75, 0x07, 0x81, 0x03, 0x05, 0x01, 0x09, 0x30, 0x26, 0xFF, 0x0F, 0x75, 0x10, 0x81, 0x02, 0x09,
            0x31, 0x26, 0xFF, 0x07, 0x81, 0x02, 0xC0,
        ];
        let l = TabletLayout::from_fields(&parse_report_descriptor(&desc).unwrap()).unwrap();
        assert_eq!((l.report_id, l.buttons.len(), l.wheel), (1, 1, None));
        let r = l.decode(&[1, 0x01, 0xFF, 0x0F, 0xFF, 0x03]).unwrap();
        assert_eq!(r, TabletReport { buttons: 1, x: 0xFFFF, y: 0x3FF * 0xFFFF / 0x7FF, wheel: 0 });
        assert_eq!(l.decode(&[2, 0, 0, 0, 0, 0]), Err(ReportError::UnknownReportId));
        // Values outside the declared range are clamped, not wrapped.
        let r = l.decode(&[1, 0x00, 0xFF, 0xFF, 0x00, 0x00]).unwrap();
        assert_eq!((r.buttons, r.x, r.y), (0, 0xFFFF, 0));
    }

    #[test]
    fn rejects_malformed_report_descriptors() {
        assert_eq!(parse_report_descriptor(&[0x05]), Err(ParseError::Truncated));
        assert_eq!(parse_report_descriptor(&[0x27, 1, 2]), Err(ParseError::Truncated));
        assert_eq!(parse_report_descriptor(&[0xC0]), Err(ParseError::BadNesting));
        assert_eq!(parse_report_descriptor(&[0xA1, 0x01]), Err(ParseError::BadNesting));
        assert_eq!(parse_report_descriptor(&[0xB4]), Err(ParseError::BadStack));
        assert_eq!(parse_report_descriptor(&[0x85, 0x00]), Err(ParseError::TooLarge), "report id 0 is reserved");
        assert_eq!(parse_report_descriptor(&[0xFE, 10, 0]), Err(ParseError::Truncated));
        let deep = [0xA1u8, 0x00].repeat(MAX_DEPTH + 1);
        assert_eq!(parse_report_descriptor(&deep), Err(ParseError::BadNesting));
        let pushes = [0xA4u8].repeat(MAX_STACK + 1);
        assert_eq!(parse_report_descriptor(&pushes), Err(ParseError::BadStack));
        // Report size beyond 32 bits.
        assert_eq!(parse_report_descriptor(&[0x75, 0x40, 0x95, 0x01, 0x81, 0x02]), Err(ParseError::TooLarge));
        // Huge report count of 1-bit data fields: bounded.
        assert_eq!(parse_report_descriptor(&[0x75, 0x01, 0x96, 0x00, 0x04, 0x81, 0x02]), Err(ParseError::TooLarge));
    }

    #[test]
    fn report_descriptor_parser_never_panics_on_mutations() {
        for i in 0..QEMU_MOUSE.len() {
            for v in [0u8, 0x01, 0x03, 0x7F, 0x80, 0xA1, 0xC0, 0xFE, 0xFF] {
                let mut d = QEMU_MOUSE;
                d[i] = v;
                if let Ok(fields) = parse_report_descriptor(&d) {
                    if let Some(l) = MouseLayout::from_fields(&fields) {
                        let _ = l.decode(&[0xFF; 8]);
                        let _ = l.decode(&[]);
                    }
                }
                let _ = parse_report_descriptor(&d[..i]);
            }
        }
    }

    #[test]
    fn field_extraction_bounds() {
        let f = Field { report_id: 0, offset: 4, size: 8, page: 1, usage: 0x30, logical_min: -127, logical_max: 127, physical_min: 0, physical_max: 0, unit: 0, unit_exponent: 0, relative: true };
        assert_eq!(f.read(&[0xF0, 0x0F]), Some(-1));
        assert_eq!(f.read(&[0xF0]), None);
        let unsigned = Field { logical_min: 0, ..f };
        assert_eq!(unsigned.read(&[0xF0, 0x0F]), Some(255));
        let wide = Field { offset: 0, size: 32, ..f };
        assert_eq!(wide.read(&[0xFF; 4]), Some(-1));
        assert_eq!(Field { size: 0, ..f }.read(&[0; 4]), None);
        assert_eq!(Field { size: 33, ..f }.read(&[0; 8]), None);
        assert_eq!(Field { offset: u32::MAX, ..f }.read(&[0; 8]), None);
    }
}
