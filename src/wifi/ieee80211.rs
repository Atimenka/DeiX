//! Структуры и разбор/сборка кадров IEEE 802.11 (Wi-Fi): management-кадры
//! (Beacon, Probe Request/Response, Authentication, Association
//! Request/Response), которые нужны для сканирования сетей и подключения,
//! плюс базовый Data-кадр для передачи полезной нагрузки (EAPOL/IP) после
//! подключения.
//!
//! Это протокольный уровень — сама физическая передача/приём кадров в
//! эфир выполняется драйвером конкретного радио-чипа (см. wifi::driver),
//! которого у нас нет в виде реального железа, поэтому этот модуль
//! проверяется unit-тестами на симметричность build/parse, а не на живом
//! эфире.

pub const MAC_ADDR_LEN: usize = 6;
pub type MacAddr = [u8; MAC_ADDR_LEN];

pub const BROADCAST: MacAddr = [0xFF; MAC_ADDR_LEN];

/// Frame Control field: type (2 бита) + subtype (4 бита) в байте 0
/// (структура согласно IEEE 802.11-2020, раздел 9.2.4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    Management(ManagementSubtype),
    Control,
    Data,
    Unknown(u8, u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagementSubtype {
    AssociationRequest,
    AssociationResponse,
    ProbeRequest,
    ProbeResponse,
    Beacon,
    Authentication,
    Deauthentication,
    Disassociation,
    Other(u8),
}

impl FrameType {
    pub fn from_frame_control(fc: u16) -> FrameType {
        let type_bits = ((fc >> 2) & 0b11) as u8;
        let subtype_bits = ((fc >> 4) & 0b1111) as u8;

        match type_bits {
            0b00 => FrameType::Management(match subtype_bits {
                0b0000 => ManagementSubtype::AssociationRequest,
                0b0001 => ManagementSubtype::AssociationResponse,
                0b0100 => ManagementSubtype::ProbeRequest,
                0b0101 => ManagementSubtype::ProbeResponse,
                0b1000 => ManagementSubtype::Beacon,
                0b1011 => ManagementSubtype::Authentication,
                0b1100 => ManagementSubtype::Deauthentication,
                0b1010 => ManagementSubtype::Disassociation,
                other => ManagementSubtype::Other(other),
            }),
            0b01 => FrameType::Control,
            0b10 => FrameType::Data,
            other => FrameType::Unknown(other, subtype_bits),
        }
    }

    pub fn to_frame_control(self) -> u16 {
        let (type_bits, subtype_bits): (u16, u16) = match self {
            FrameType::Management(sub) => (
                0b00,
                match sub {
                    ManagementSubtype::AssociationRequest => 0b0000,
                    ManagementSubtype::AssociationResponse => 0b0001,
                    ManagementSubtype::ProbeRequest => 0b0100,
                    ManagementSubtype::ProbeResponse => 0b0101,
                    ManagementSubtype::Beacon => 0b1000,
                    ManagementSubtype::Authentication => 0b1011,
                    ManagementSubtype::Deauthentication => 0b1100,
                    ManagementSubtype::Disassociation => 0b1010,
                    ManagementSubtype::Other(v) => v as u16,
                },
            ),
            FrameType::Control => (0b01, 0),
            FrameType::Data => (0b10, 0),
            FrameType::Unknown(t, s) => (t as u16, s as u16),
        };
        (type_bits << 2) | (subtype_bits << 4)
    }
}

/// Стандартный 24-байтный заголовок management/data-кадра (без QoS-поля).
pub struct FrameHeader {
    pub frame_type: FrameType,
    pub duration: u16,
    pub addr1: MacAddr, // получатель (DA/RA)
    pub addr2: MacAddr, // отправитель (SA/TA)
    pub addr3: MacAddr, // BSSID (в большинстве случаев)
    pub seq_ctrl: u16,
}

pub const HEADER_LEN: usize = 24;

impl FrameHeader {
    pub fn parse(data: &[u8]) -> Option<(FrameHeader, &[u8])> {
        if data.len() < HEADER_LEN {
            return None;
        }
        let fc = u16::from_le_bytes([data[0], data[1]]);
        let duration = u16::from_le_bytes([data[2], data[3]]);
        let mut addr1 = [0u8; 6];
        let mut addr2 = [0u8; 6];
        let mut addr3 = [0u8; 6];
        addr1.copy_from_slice(&data[4..10]);
        addr2.copy_from_slice(&data[10..16]);
        addr3.copy_from_slice(&data[16..22]);
        let seq_ctrl = u16::from_le_bytes([data[22], data[23]]);

        let header = FrameHeader {
            frame_type: FrameType::from_frame_control(fc),
            duration,
            addr1,
            addr2,
            addr3,
            seq_ctrl,
        };
        Some((header, &data[HEADER_LEN..]))
    }

    pub fn build(&self, out: &mut [u8]) -> usize {
        let fc = self.frame_type.to_frame_control();
        out[0..2].copy_from_slice(&fc.to_le_bytes());
        out[2..4].copy_from_slice(&self.duration.to_le_bytes());
        out[4..10].copy_from_slice(&self.addr1);
        out[10..16].copy_from_slice(&self.addr2);
        out[16..22].copy_from_slice(&self.addr3);
        out[22..24].copy_from_slice(&self.seq_ctrl.to_le_bytes());
        HEADER_LEN
    }
}

/// Element ID для тегированных параметров в management-кадрах (IEEE
/// 802.11-2020, таблица 9-92). Нас интересуют минимум SSID и RSN
/// (Robust Security Network — несёт информацию о поддерживаемых
/// cipher suite для WPA2).
pub const ELEMENT_ID_SSID: u8 = 0;
pub const ELEMENT_ID_SUPPORTED_RATES: u8 = 1;
pub const ELEMENT_ID_RSN: u8 = 48;

/// Разбирает TLV-элементы (тег, длина, данные) из тела Beacon/Probe Response.
pub struct ElementIterator<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> ElementIterator<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        ElementIterator { data, offset: 0 }
    }
}

impl<'a> Iterator for ElementIterator<'a> {
    type Item = (u8, &'a [u8]);

    fn next(&mut self) -> Option<(u8, &'a [u8])> {
        if self.offset + 2 > self.data.len() {
            return None;
        }
        let id = self.data[self.offset];
        let len = self.data[self.offset + 1] as usize;
        let start = self.offset + 2;
        if start + len > self.data.len() {
            return None;
        }
        let value = &self.data[start..start + len];
        self.offset = start + len;
        Some((id, value))
    }
}

/// Информация о сети, извлечённая из Beacon/Probe Response — то, что видит
/// пользователь при `wifi scan`.
pub struct ScannedNetwork {
    pub bssid: MacAddr,
    pub ssid: [u8; 32],
    pub ssid_len: usize,
    pub rsn_present: bool, // есть ли WPA2 (RSN IE)
    pub channel: u8,
    pub signal_strength: i8, // dBm, приблизительно
}

impl ScannedNetwork {
    pub fn ssid_str(&self) -> &str {
        core::str::from_utf8(&self.ssid[..self.ssid_len]).unwrap_or("?")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frame_control_roundtrip() {
        let ft = FrameType::Management(ManagementSubtype::Beacon);
        let fc = ft.to_frame_control();
        let parsed = FrameType::from_frame_control(fc);
        assert_eq!(ft, parsed);
    }

    #[test]
    fn test_header_roundtrip() {
        let header = FrameHeader {
            frame_type: FrameType::Management(ManagementSubtype::ProbeRequest),
            duration: 0x1234,
            addr1: [1, 2, 3, 4, 5, 6],
            addr2: [7, 8, 9, 10, 11, 12],
            addr3: [13, 14, 15, 16, 17, 18],
            seq_ctrl: 0xABCD,
        };

        let mut buf = [0u8; HEADER_LEN];
        header.build(&mut buf);

        let (parsed, rest) = FrameHeader::parse(&buf).unwrap();
        assert_eq!(rest.len(), 0);
        assert_eq!(parsed.duration, 0x1234);
        assert_eq!(parsed.addr1, [1, 2, 3, 4, 5, 6]);
        assert_eq!(parsed.addr2, [7, 8, 9, 10, 11, 12]);
        assert_eq!(parsed.addr3, [13, 14, 15, 16, 17, 18]);
        assert_eq!(parsed.seq_ctrl, 0xABCD);
        assert_eq!(
            parsed.frame_type,
            FrameType::Management(ManagementSubtype::ProbeRequest)
        );
    }

    #[test]
    fn test_element_iterator() {
        // SSID "Hi" (2 байта) + RSN placeholder (1 байт 0xAA)
        let data = [0u8, 2, b'H', b'i', 48, 1, 0xAA];
        let mut iter = ElementIterator::new(&data);

        let (id1, val1) = iter.next().unwrap();
        assert_eq!(id1, ELEMENT_ID_SSID);
        assert_eq!(val1, b"Hi");

        let (id2, val2) = iter.next().unwrap();
        assert_eq!(id2, ELEMENT_ID_RSN);
        assert_eq!(val2, &[0xAA]);

        assert!(iter.next().is_none());
    }
}
