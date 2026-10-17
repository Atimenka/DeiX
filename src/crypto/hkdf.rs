//! HMAC-SHA256 (RFC 2104) и HKDF (RFC 5869) — схема вывода ключей TLS 1.3.

use super::sha256::Sha256;
use alloc::vec::Vec;

const BLOCK: usize = 64;

/// HMAC-SHA256 от конкатенации частей сообщения (без промежуточных копий).
pub fn hmac_sha256_parts(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        let mut h = Sha256::new();
        h.update(key);
        k[..32].copy_from_slice(&h.finalize());
    } else {
        k[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }

    let mut inner = Sha256::new();
    inner.update(&ipad);
    for p in parts {
        inner.update(p);
    }
    let inner_hash = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(&opad);
    outer.update(&inner_hash);
    outer.finalize()
}

pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    hmac_sha256_parts(key, &[data])
}

/// HKDF-Extract: PRK = HMAC(salt, ikm).
pub fn hkdf_extract(salt: &[u8], ikm: &[u8]) -> [u8; 32] {
    hmac_sha256(salt, ikm)
}

/// HKDF-Expand: OKM длиной out_len (не более 255*32).
pub fn hkdf_expand(prk: &[u8; 32], info: &[u8], out_len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(out_len);
    let mut t: [u8; 32] = [0; 32];
    let mut block_idx: u8 = 0;
    while out.len() < out_len {
        block_idx += 1;
        let t_prev: &[u8] = if block_idx == 1 { &[] } else { &t };
        t = hmac_sha256_parts(prk, &[t_prev, info, &[block_idx]]);
        let take = (out_len - out.len()).min(32);
        out.extend_from_slice(&t[..take]);
    }
    out
}
