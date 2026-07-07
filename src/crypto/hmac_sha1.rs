//! HMAC-SHA1 (RFC 2104 / FIPS 198-1). Используется и в PBKDF2 (для деривации
//! PMK из пароля Wi-Fi), и напрямую в PRF (для деривации PTK/GTK по
//! IEEE 802.11i Annex H).

use super::sha1::{Sha1, DIGEST_LEN};

const BLOCK_LEN: usize = 64;

pub fn hmac_sha1(key: &[u8], message: &[u8]) -> [u8; DIGEST_LEN] {
    // Если ключ длиннее блока — сначала хэшируем его.
    let mut key_block = [0u8; BLOCK_LEN];
    if key.len() > BLOCK_LEN {
        let hashed = super::sha1::sha1(key);
        key_block[..DIGEST_LEN].copy_from_slice(&hashed);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0x36u8; BLOCK_LEN];
    let mut opad = [0x5cu8; BLOCK_LEN];
    for i in 0..BLOCK_LEN {
        ipad[i] ^= key_block[i];
        opad[i] ^= key_block[i];
    }

    let mut inner = Sha1::new();
    inner.update(&ipad);
    inner.update(message);
    let inner_digest = inner.finalize();

    let mut outer = Sha1::new();
    outer.update(&opad);
    outer.update(&inner_digest);
    outer.finalize()
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
    fn test_rfc2202_case1() {
        // RFC 2202 Test Case 1: key = 20 * 0x0b, data = "Hi There"
        let key = [0x0bu8; 20];
        let digest = hmac_sha1(&key, b"Hi There");
        assert_eq!(to_hex(&digest), "b617318655057264e28bc0b6fb378c8ef146be00");
    }

    #[test]
    fn test_rfc2202_case2() {
        // RFC 2202 Test Case 2: key = "Jefe", data = "what do ya want for nothing?"
        let digest = hmac_sha1(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(to_hex(&digest), "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79");
    }
}
