//! EAPOL-Key кадры (IEEE 802.1X / 802.11i) — именно ими обмениваются
//! точка доступа и клиент во время WPA2 4-way handshake, чтобы
//! согласовать PTK/GTK, не передавая пароль по эфиру.

// Размер тела EAPOL-Key фрейма без Key Data (IEEE 802.11-2020, рис. 12-35):
// descriptor_type(1) + key_info(2) + key_length(2) + replay_counter(8) +
// key_nonce(32) + key_iv(16) + key_rsc(8) + reserved(8) + key_mic(16) +
// key_data_len(2) = 95 байт.
pub const EAPOL_KEY_FRAME_LEN: usize = 95;
pub const MIC_LEN: usize = 16;

/// Key Information field flags (IEEE 802.11-2020, раздел 12.7.2).
pub const KEY_INFO_KEY_TYPE_PAIRWISE: u16 = 1 << 3;
pub const KEY_INFO_INSTALL: u16 = 1 << 6;
pub const KEY_INFO_ACK: u16 = 1 << 7;
pub const KEY_INFO_MIC: u16 = 1 << 8;
pub const KEY_INFO_SECURE: u16 = 1 << 9;

pub struct EapolKeyFrame {
    pub descriptor_type: u8, // 2 = RSN (WPA2)
    pub key_info: u16,
    pub key_length: u16,
    pub replay_counter: u64,
    pub key_nonce: [u8; 32],
    pub key_mic: [u8; MIC_LEN],
    pub key_data: alloc::vec::Vec<u8>,
}

impl EapolKeyFrame {
    /// Разбирает EAPOL-Key фрейм из тела Data-кадра (уже без заголовков
    /// 802.11/LLC — просто начиная с 802.1X заголовка).
    pub fn parse(data: &[u8]) -> Option<EapolKeyFrame> {
        // 802.1X header: version(1) + type(1) + length(2) = 4 байта, затем
        // сам EAPOL-Key body.
        if data.len() < 4 + EAPOL_KEY_FRAME_LEN {
            return None;
        }
        let eapol_type = data[1];
        if eapol_type != 3 {
            return None; // не EAPOL-Key
        }

        let body = &data[4..];
        let descriptor_type = body[0];
        let key_info = u16::from_be_bytes([body[1], body[2]]);
        let key_length = u16::from_be_bytes([body[3], body[4]]);
        let replay_counter = u64::from_be_bytes(body[5..13].try_into().ok()?);
        let mut key_nonce = [0u8; 32];
        key_nonce.copy_from_slice(&body[13..45]);
        // 45..61 = Key IV (не используем), 61..69 = Key RSC, 69..77 = reserved
        let mut key_mic = [0u8; MIC_LEN];
        key_mic.copy_from_slice(&body[77..93]);
        let key_data_len = u16::from_be_bytes([body[93], body[94]]) as usize;

        let key_data_start = 4 + 95;
        if data.len() < key_data_start + key_data_len {
            return None;
        }
        let key_data = data[key_data_start..key_data_start + key_data_len].to_vec();

        Some(EapolKeyFrame {
            descriptor_type,
            key_info,
            key_length,
            replay_counter,
            key_nonce,
            key_mic,
            key_data,
        })
    }

    /// Собирает EAPOL-Key фрейм. Если `mic_key` задан — считает и
    /// вписывает настоящий MIC (HMAC-SHA1-128 по KCK), иначе оставляет
    /// поле MIC нулевым (для первого сообщения от AP, где ещё нет ключей).
    pub fn build(&self, out: &mut alloc::vec::Vec<u8>, mic_key: Option<&[u8]>) {
        out.push(1); // 802.1X version
        out.push(3); // type = Key

        let body_len = 95 + self.key_data.len();
        out.extend_from_slice(&(body_len as u16).to_be_bytes());

        let body_start = out.len();

        out.push(self.descriptor_type);
        out.extend_from_slice(&self.key_info.to_be_bytes());
        out.extend_from_slice(&self.key_length.to_be_bytes());
        out.extend_from_slice(&self.replay_counter.to_be_bytes());
        out.extend_from_slice(&self.key_nonce);
        out.extend_from_slice(&[0u8; 16]); // Key IV
        out.extend_from_slice(&[0u8; 8]); // Key RSC
        out.extend_from_slice(&[0u8; 8]); // reserved
        out.extend_from_slice(&[0u8; MIC_LEN]); // MIC placeholder (заполним ниже)
        out.extend_from_slice(&(self.key_data.len() as u16).to_be_bytes());
        out.extend_from_slice(&self.key_data);

        if let Some(kck) = mic_key {
            let body = &out[body_start..];
            let mic = crate::crypto::hmac_sha1::hmac_sha1(kck, body);
            let mic_offset = body_start + 1 + 2 + 2 + 8 + 32 + 16 + 8 + 8;
            out[mic_offset..mic_offset + MIC_LEN].copy_from_slice(&mic[..MIC_LEN]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_eapol_roundtrip_without_mic() {
        let frame = EapolKeyFrame {
            descriptor_type: 2,
            key_info: KEY_INFO_KEY_TYPE_PAIRWISE | KEY_INFO_ACK,
            key_length: 16,
            replay_counter: 1,
            key_nonce: [0x42; 32],
            key_mic: [0; MIC_LEN],
            key_data: alloc::vec::Vec::new(),
        };

        let mut buf = alloc::vec::Vec::new();
        frame.build(&mut buf, None);

        let parsed = EapolKeyFrame::parse(&buf).unwrap();
        assert_eq!(parsed.descriptor_type, 2);
        assert_eq!(parsed.key_info, KEY_INFO_KEY_TYPE_PAIRWISE | KEY_INFO_ACK);
        assert_eq!(parsed.replay_counter, 1);
        assert_eq!(parsed.key_nonce, [0x42; 32]);
    }

    #[test]
    fn test_eapol_mic_is_computed() {
        let frame = EapolKeyFrame {
            descriptor_type: 2,
            key_info: KEY_INFO_KEY_TYPE_PAIRWISE,
            key_length: 16,
            replay_counter: 1,
            key_nonce: [0x11; 32],
            key_mic: [0; MIC_LEN],
            key_data: alloc::vec::Vec::new(),
        };

        let kck = [0xAAu8; 16];
        let mut buf = alloc::vec::Vec::new();
        frame.build(&mut buf, Some(&kck));

        let parsed = EapolKeyFrame::parse(&buf).unwrap();
        // MIC не должен остаться нулевым после подписи настоящим ключом.
        assert_ne!(parsed.key_mic, [0u8; MIC_LEN]);
    }
}
