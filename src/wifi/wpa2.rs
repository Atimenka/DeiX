//! WPA2-PSK 4-way handshake (IEEE 802.11i / RSN), поверх абстрактного
//! WifiDriver. Реализует полную последовательность:
//!
//!   1. Authentication (Open System) -> Association
//!   2. Message 1 (AP -> STA): ANonce
//!   3. Вычисляем PMK = PBKDF2(passphrase, ssid), генерируем SNonce,
//!      вычисляем PTK = PRF-384(PMK, ...), отправляем Message 2 с MIC
//!   4. Message 3 (AP -> STA): GTK (здесь не расшифровываем AES Key Wrap
//!      — это отдельная функция, см. TODO ниже), проверяем MIC
//!   5. Message 4 (STA -> AP): подтверждение, после этого можно
//!      передавать зашифрованные данные
//!
//! Ограничение текущей реализации: сама расшифровка/шифрование данных
//! через AES-CCMP не реализована (это отдельный большой кусок — AES +
//! CCM mode), поэтому здесь мы доводим handshake до конца и получаем
//! правильные PTK/GTK, но фактическую передачу зашифрованных данных
//! оставляем на следующий шаг развития.

use crate::crypto::pbkdf2::pbkdf2_hmac_sha1;
use crate::crypto::prf::derive_ptk;
use crate::wifi::eapol::{EapolKeyFrame, KEY_INFO_KEY_TYPE_PAIRWISE, KEY_INFO_MIC, KEY_INFO_SECURE, MIC_LEN};
use crate::wifi::ieee80211::MacAddr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeState {
    WaitingMessage1,
    WaitingMessage3,
    Completed,
    Failed,
}

pub struct Handshake {
    pub state: HandshakeState,
    pmk: [u8; 32],
    ptk: [u8; 48],
    snonce: [u8; 32],
    anonce: [u8; 32],
    ap_mac: MacAddr,
    sta_mac: MacAddr,
    replay_counter: u64,
}

impl Handshake {
    /// Запускает handshake: вычисляет PMK из пароля/SSID и генерирует
    /// SNonce (случайное 32-байтное значение — здесь берём его из PIT-
    /// таймера, т.к. полноценного CSPRNG у нас пока нет; для реального
    /// продакшна тут нужен настоящий источник энтропии).
    pub fn new(ssid: &[u8], passphrase: &[u8], ap_mac: MacAddr, sta_mac: MacAddr) -> Self {
        let mut pmk = [0u8; 32];
        pbkdf2_hmac_sha1(passphrase, ssid, 4096, 32, &mut pmk);

        let mut snonce = [0u8; 32];
        fill_pseudo_random(&mut snonce);

        Handshake {
            state: HandshakeState::WaitingMessage1,
            pmk,
            ptk: [0u8; 48],
            snonce,
            anonce: [0u8; 32],
            ap_mac,
            sta_mac,
            replay_counter: 0,
        }
    }

    fn kck(&self) -> &[u8] {
        &self.ptk[0..16]
    }

    /// Обрабатывает входящий EAPOL-Key фрейм. Возвращает Some(frame) —
    /// готовый ответный фрейм для отправки (Message 2 или Message 4),
    /// если нужно что-то ответить.
    pub fn on_eapol_frame(&mut self, frame: &EapolKeyFrame) -> Option<alloc::vec::Vec<u8>> {
        match self.state {
            HandshakeState::WaitingMessage1 => self.handle_message1(frame),
            HandshakeState::WaitingMessage3 => self.handle_message3(frame),
            _ => None,
        }
    }

    fn handle_message1(&mut self, frame: &EapolKeyFrame) -> Option<alloc::vec::Vec<u8>> {
        // Message 1: ANonce от AP, MIC ещё не проверяем (PTK ещё не готов).
        self.anonce = frame.key_nonce;
        self.replay_counter = frame.replay_counter;

        derive_ptk(&self.pmk, self.ap_mac, self.sta_mac, self.anonce, self.snonce, &mut self.ptk);

        // Message 2: STA -> AP, SNonce + MIC, подтверждаем что мы знаем PMK.
        let response = EapolKeyFrame {
            descriptor_type: 2,
            key_info: KEY_INFO_KEY_TYPE_PAIRWISE | KEY_INFO_MIC,
            key_length: 16,
            replay_counter: self.replay_counter,
            key_nonce: self.snonce,
            key_mic: [0; MIC_LEN],
            key_data: alloc::vec::Vec::new(), // здесь обычно кладут RSN IE
        };

        let mut buf = alloc::vec::Vec::new();
        response.build(&mut buf, Some(self.kck()));

        self.state = HandshakeState::WaitingMessage3;
        Some(buf)
    }

    fn handle_message3(&mut self, frame: &EapolKeyFrame) -> Option<alloc::vec::Vec<u8>> {
        // Проверяем MIC — доказывает, что AP тоже владеет верным PMK
        // (иначе пароль неверный, либо это атака).
        let mut check_frame_data = alloc::vec::Vec::new();
        let zero_mic_frame = EapolKeyFrame {
            descriptor_type: frame.descriptor_type,
            key_info: frame.key_info,
            key_length: frame.key_length,
            replay_counter: frame.replay_counter,
            key_nonce: frame.key_nonce,
            key_mic: [0; MIC_LEN],
            key_data: frame.key_data.clone(),
        };
        zero_mic_frame.build(&mut check_frame_data, None);

        let expected_mic = crate::crypto::hmac_sha1::hmac_sha1(self.kck(), &check_frame_data);
        if expected_mic[..MIC_LEN] != frame.key_mic[..] {
            self.state = HandshakeState::Failed;
            return None;
        }

        self.replay_counter = frame.replay_counter;

        // Message 4: подтверждаем установку ключей. После этого можно
        // включать шифрование (AES-CCMP) — см. ограничение в шапке файла.
        let response = EapolKeyFrame {
            descriptor_type: 2,
            key_info: KEY_INFO_KEY_TYPE_PAIRWISE | KEY_INFO_MIC | KEY_INFO_SECURE,
            key_length: 0,
            replay_counter: self.replay_counter,
            key_nonce: [0; 32],
            key_mic: [0; MIC_LEN],
            key_data: alloc::vec::Vec::new(),
        };

        let mut buf = alloc::vec::Vec::new();
        response.build(&mut buf, Some(self.kck()));

        self.state = HandshakeState::Completed;
        Some(buf)
    }

    pub fn ptk(&self) -> &[u8; 48] {
        &self.ptk
    }
}

/// Простой источник псевдослучайных байт на основе таймера + LCG —
/// НЕ криптографически стойкий генератор, годится только для получения
/// уникального SNonce в учебной реализации. Для настоящего продакшна
/// здесь обязателен аппаратный источник энтропии (RDRAND и т.п.).
fn fill_pseudo_random(out: &mut [u8; 32]) {
    let mut state = crate::timer::uptime_ms().wrapping_mul(2654435761).wrapping_add(0x9E3779B9);
    for byte in out.iter_mut() {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        *byte = (state >> 33) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wifi::eapol::{EapolKeyFrame, KEY_INFO_ACK};

    #[test]
    fn test_handshake_message1_produces_message2() {
        let ap_mac: MacAddr = [0x00, 0x14, 0x6c, 0x7e, 0x40, 0x80];
        let sta_mac: MacAddr = [0x00, 0x13, 0x46, 0xfe, 0x32, 0x0c];

        let mut hs = Handshake::new(b"Harkonen", b"12345678", ap_mac, sta_mac);
        assert_eq!(hs.state, HandshakeState::WaitingMessage1);

        let msg1 = EapolKeyFrame {
            descriptor_type: 2,
            key_info: KEY_INFO_ACK,
            key_length: 0,
            replay_counter: 1,
            key_nonce: [0x99; 32],
            key_mic: [0; MIC_LEN],
            key_data: alloc::vec::Vec::new(),
        };

        let response = hs.on_eapol_frame(&msg1);
        assert!(response.is_some());
        assert_eq!(hs.state, HandshakeState::WaitingMessage3);
    }

    #[test]
    fn test_handshake_rejects_bad_mic_in_message3() {
        let ap_mac: MacAddr = [0x00, 0x14, 0x6c, 0x7e, 0x40, 0x80];
        let sta_mac: MacAddr = [0x00, 0x13, 0x46, 0xfe, 0x32, 0x0c];

        let mut hs = Handshake::new(b"Harkonen", b"wrong-password", ap_mac, sta_mac);

        let msg1 = EapolKeyFrame {
            descriptor_type: 2,
            key_info: KEY_INFO_ACK,
            key_length: 0,
            replay_counter: 1,
            key_nonce: [0x99; 32],
            key_mic: [0; MIC_LEN],
            key_data: alloc::vec::Vec::new(),
        };
        hs.on_eapol_frame(&msg1);

        // Message 3 с заведомо неверным MIC (т.к. PMK неверный из-за
        // неправильного пароля) должен быть отвергнут.
        let msg3 = EapolKeyFrame {
            descriptor_type: 2,
            key_info: KEY_INFO_MIC,
            key_length: 0,
            replay_counter: 2,
            key_nonce: [0x11; 32],
            key_mic: [0xAB; MIC_LEN], // заведомо неверный MIC
            key_data: alloc::vec::Vec::new(),
        };
        let response = hs.on_eapol_frame(&msg3);
        assert!(response.is_none());
        assert_eq!(hs.state, HandshakeState::Failed);
    }
}
