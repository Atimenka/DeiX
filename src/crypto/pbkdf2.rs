//! PBKDF2-HMAC-SHA1 (RFC 2898). В WPA2-PSK именно так вычисляется PMK
//! (Pairwise Master Key) из пароля сети и SSID:
//!
//!   PMK = PBKDF2(HMAC-SHA1, passphrase, ssid, 4096, 256 бит)
//!
//! См. IEEE 802.11i Annex H.4.1.

use super::hmac_sha1::hmac_sha1;
use super::sha1::DIGEST_LEN;

/// dk_len — желаемая длина выходного ключа в байтах (для WPA2 PMK это 32).
pub fn pbkdf2_hmac_sha1(password: &[u8], salt: &[u8], iterations: u32, dk_len: usize, out: &mut [u8]) {
    assert!(out.len() >= dk_len);

    let num_blocks = (dk_len + DIGEST_LEN - 1) / DIGEST_LEN;
    let mut produced = 0usize;

    for block_index in 1..=num_blocks as u32 {
        let mut salt_with_index = alloc::vec::Vec::with_capacity(salt.len() + 4);
        salt_with_index.extend_from_slice(salt);
        salt_with_index.extend_from_slice(&block_index.to_be_bytes());

        let mut u = hmac_sha1(password, &salt_with_index);
        let mut result = u;

        for _ in 1..iterations {
            u = hmac_sha1(password, &u);
            for i in 0..DIGEST_LEN {
                result[i] ^= u[i];
            }
        }

        let take = (dk_len - produced).min(DIGEST_LEN);
        out[produced..produced + take].copy_from_slice(&result[..take]);
        produced += take;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_hex(bytes: &[u8]) -> alloc::string::String {
        use core::fmt::Write;
        let mut s = alloc::string::String::new();
        for b in bytes {
            write!(s, "{:02x}", b).unwrap();
        }
        s
    }

    #[test]
    fn test_wpa2_pmk_known_vector() {
        // Известный тестовый вектор из документации WPA2 (aircrack-ng /
        // многочисленные справочники): passphrase="password",
        // ssid="IEEE" -> PMK (32 байта).
        let mut pmk = [0u8; 32];
        pbkdf2_hmac_sha1(b"password", b"IEEE", 4096, 32, &mut pmk);
        assert_eq!(
            to_hex(&pmk),
            "f42c6fc52df0ebef9ebb4b90b38a5f902e83fe1b135a70e23aed762e9710a12e"
        );
    }
}
