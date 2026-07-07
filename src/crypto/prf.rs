//! PRF (Pseudo-Random Function) по IEEE 802.11i Annex H.3. Используется
//! для деривации PTK (Pairwise Transient Key) из PMK во время 4-way
//! handshake WPA2.
//!
//! Алгоритм:
//!   R = ""
//!   for i in 0..=(len_bits + 159) / 160:
//!       R = R || HMAC-SHA1(K, A || 0x00 || B || i)
//!   return R[0..len_bits/8]
//!
//! где K — ключ (PMK), A — метка ("Pairwise key expansion"), B — контекст
//! (MAC-адреса + nonce'ы в каноническом порядке min/max), i — однобайтовый
//! счётчик итерации.

use super::hmac_sha1::hmac_sha1;
use super::sha1::DIGEST_LEN;

/// PRF(K, A, B, len_bytes) — возвращает len_bytes байт псевдослучайных
/// данных в `out` (out.len() должен быть >= len_bytes).
pub fn prf(key: &[u8], label: &[u8], context: &[u8], len_bytes: usize, out: &mut [u8]) {
    assert!(out.len() >= len_bytes);

    let num_iterations = (len_bytes * 8 + DIGEST_LEN * 8 - 1) / (DIGEST_LEN * 8);
    let mut produced = 0usize;

    let mut message = alloc::vec::Vec::with_capacity(label.len() + 1 + context.len() + 1);
    message.extend_from_slice(label);
    message.push(0x00);
    message.extend_from_slice(context);
    message.push(0); // счётчик итерации, обновляется ниже

    let counter_pos = message.len() - 1;

    for i in 0..num_iterations {
        message[counter_pos] = i as u8;
        let digest = hmac_sha1(key, &message);

        let take = (len_bytes - produced).min(DIGEST_LEN);
        out[produced..produced + take].copy_from_slice(&digest[..take]);
        produced += take;
    }
}

/// PTK по IEEE 802.11i: PRF-384 для CCMP/AES (384 бита = 48 байт: 16 KCK
/// + 16 KEK + 16 TK) или PRF-512 для TKIP (у нас поддерживаем только CCMP,
/// поэтому всегда 384 бита = 48 байт).
///
/// Контекст B собирается как:
///   min(AA,SPA) || max(AA,SPA) || min(ANonce,SNonce) || max(ANonce,SNonce)
/// где AA — MAC точки доступа, SPA — MAC клиента (нашей карты).
pub fn derive_ptk(pmk: &[u8], ap_mac: [u8; 6], sta_mac: [u8; 6], anonce: [u8; 32], snonce: [u8; 32], out: &mut [u8; 48]) {
    let mut context = alloc::vec::Vec::with_capacity(6 + 6 + 32 + 32);

    if ap_mac <= sta_mac {
        context.extend_from_slice(&ap_mac);
        context.extend_from_slice(&sta_mac);
    } else {
        context.extend_from_slice(&sta_mac);
        context.extend_from_slice(&ap_mac);
    }

    if anonce <= snonce {
        context.extend_from_slice(&anonce);
        context.extend_from_slice(&snonce);
    } else {
        context.extend_from_slice(&snonce);
        context.extend_from_slice(&anonce);
    }

    prf(pmk, b"Pairwise key expansion", &context, 48, out);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::pbkdf2::pbkdf2_hmac_sha1;

    fn to_hex(bytes: &[u8]) -> alloc::string::String {
        use core::fmt::Write;
        let mut s = alloc::string::String::new();
        for b in bytes {
            write!(s, "{:02x}", b).unwrap();
        }
        s
    }

    fn from_hex(s: &str) -> alloc::vec::Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn test_ptk_real_capture_vector() {
        // Реальный численный пример полного WPA2 4-way handshake (взят из
        // независимого разбора с настоящим MIC для проверки): SSID
        // "Harkonen", passphrase "12345678". Вектор включает PSK, SSID,
        // ANonce, SNonce, MAC-адреса AP/клиента и настоящий MIC второго
        // EAPOL-кадра — то есть проверяет всю цепочку PBKDF2 -> PMK -> PRF
        // -> PTK -> HMAC-MIC целиком, а не только отдельные примитивы.
        let mut pmk = [0u8; 32];
        pbkdf2_hmac_sha1(b"12345678", b"Harkonen", 4096, 32, &mut pmk);
        assert_eq!(
            to_hex(&pmk),
            "ee51883793a6f68e9615fe73c80a3aa6f2dd0ea537bce627b929183cc6e57925"
        );

        let anonce_vec = from_hex("225854b0444de3af06d1492b852984f04cf6274c0e3218b8681756864db7a055");
        let snonce_vec = from_hex("59168bc3a5df18d71efb6423f340088dab9e1ba2bbc58659e07b3764b0de8570");
        let mut anonce = [0u8; 32];
        let mut snonce = [0u8; 32];
        anonce.copy_from_slice(&anonce_vec);
        snonce.copy_from_slice(&snonce_vec);

        let ap_mac: [u8; 6] = [0x00, 0x14, 0x6c, 0x7e, 0x40, 0x80];
        let sta_mac: [u8; 6] = [0x00, 0x13, 0x46, 0xfe, 0x32, 0x0c];

        let mut ptk = [0u8; 48];
        derive_ptk(&pmk, ap_mac, sta_mac, anonce, snonce, &mut ptk);

        let kck = &ptk[0..16];

        // Второй EAPOL-Key кадр handshake с обнулённым полем MIC — MIC
        // должен совпасть с тем, что реально было передано по эфиру.
        let eapol_frame_zeroed_mic = from_hex("0103007502010a0010000000000000000159168bc3a5df18d71efb6423f340088dab9e1ba2bbc58659e07b3764b0de8570000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001630140100000fac040100000fac040100000fac020100");

        let mic = crate::crypto::hmac_sha1::hmac_sha1(kck, &eapol_frame_zeroed_mic);
        assert_eq!(to_hex(&mic[..16]), "d5355382b8a9b806dcaf99cdaf564eb6");
    }
}
