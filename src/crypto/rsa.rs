//! Проверка подписей RSA (RFC 8017): RSASSA-PKCS1-v1_5 (подписи в
//! цепочках X.509) и RSASSA-PSS с SHA-256 (CertificateVerify в TLS 1.3,
//! схема rsa_pss_rsae_sha256). Только верификация — закрытых ключей в
//! системе нет.

use super::bigint::BigUint;
use super::sha256::sha256;
use alloc::vec::Vec;

/// DigestInfo-префиксы PKCS#1 v1.5 (RFC 8017 §9.2, примечание 1).
const DI_SHA256: &[u8] = &[
    0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03,
    0x04, 0x02, 0x01, 0x05, 0x00, 0x04, 0x20,
];
const DI_SHA384: &[u8] = &[
    0x30, 0x41, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03,
    0x04, 0x02, 0x02, 0x05, 0x00, 0x04, 0x30,
];
const DI_SHA512: &[u8] = &[
    0x30, 0x51, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03,
    0x04, 0x02, 0x03, 0x05, 0x00, 0x04, 0x40,
];

/// RSAVP1: EM = sig^e mod n, выровненный к длине k байт.
fn rsavp1(n: &[u8], e: &[u8], sig: &[u8], out_len: usize) -> Option<Vec<u8>> {
    let n_int = BigUint::from_be_bytes(n);
    let e_int = BigUint::from_be_bytes(e);
    let s_int = BigUint::from_be_bytes(sig);
    if n_int.is_zero() || e_int.is_zero() {
        return None;
    }
    if s_int.cmp_big(&n_int) != core::cmp::Ordering::Less {
        return None;
    }
    s_int.mod_pow(&e_int, &n_int).to_be_bytes_padded(out_len)
}

/// Длина модуля в байтах (k из RFC 8017).
fn modulus_len(n: &[u8]) -> usize {
    (BigUint::from_be_bytes(n).bit_len() + 7) / 8
}

/// RSASSA-PKCS1-v1_5: EM = 00 01 FF..FF 00 || DigestInfo || hash.
fn verify_pkcs1(n: &[u8], e: &[u8], sig: &[u8], digest_info: &[u8], hash: &[u8]) -> bool {
    let k = modulus_len(n);
    if k < digest_info.len() + hash.len() + 11 {
        return false;
    }
    let em = match rsavp1(n, e, sig, k) {
        Some(v) => v,
        None => return false,
    };
    if em[0] != 0x00 || em[1] != 0x01 {
        return false;
    }
    let ps_end = k - digest_info.len() - hash.len() - 1;
    if em[2..ps_end].iter().any(|&b| b != 0xff) {
        return false;
    }
    if em[ps_end] != 0x00 {
        return false;
    }
    em[ps_end + 1..ps_end + 1 + digest_info.len()] == *digest_info
        && em[k - hash.len()..] == *hash
}

pub fn verify_pkcs1_sha256(n: &[u8], e: &[u8], sig: &[u8], hash: &[u8; 32]) -> bool {
    verify_pkcs1(n, e, sig, DI_SHA256, hash)
}

pub fn verify_pkcs1_sha384(n: &[u8], e: &[u8], sig: &[u8], hash: &[u8; 48]) -> bool {
    verify_pkcs1(n, e, sig, DI_SHA384, hash)
}

pub fn verify_pkcs1_sha512(n: &[u8], e: &[u8], sig: &[u8], hash: &[u8; 64]) -> bool {
    verify_pkcs1(n, e, sig, DI_SHA512, hash)
}

/// MGF1 на SHA-256 (RFC 8017 §B.2.1).
fn mgf1_sha256(seed: &[u8], out_len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(out_len + 32);
    let mut counter: u32 = 0;
    while out.len() < out_len {
        let mut block = Vec::with_capacity(seed.len() + 4);
        block.extend_from_slice(seed);
        block.extend_from_slice(&counter.to_be_bytes());
        out.extend_from_slice(&sha256(&block));
        counter += 1;
    }
    out.truncate(out_len);
    out
}

/// EMSA-PSS-VERIFY c SHA-256, MGF1-SHA256, sLen = 32
/// (параметры rsa_pss_rsae_sha256 из RFC 8446 §4.2.3).
pub fn verify_pss_sha256(n: &[u8], e: &[u8], sig: &[u8], m_hash: &[u8; 32]) -> bool {
    const H_LEN: usize = 32;
    const S_LEN: usize = 32;
    let mod_bits = BigUint::from_be_bytes(n).bit_len();
    if mod_bits < 2 {
        return false;
    }
    let em_bits = mod_bits - 1;
    let em_len = (em_bits + 7) / 8;
    if em_len < H_LEN + S_LEN + 2 {
        return false;
    }
    let em = match rsavp1(n, e, sig, em_len) {
        Some(v) => v,
        None => return false,
    };
    if em[em_len - 1] != 0xbc {
        return false;
    }
    let db_len = em_len - H_LEN - 1;
    let masked_db = &em[..db_len];
    let h = &em[db_len..db_len + H_LEN];

    // Верхние (8*emLen - emBits) бит EM обязаны быть нулями.
    let top_bits = 8 * em_len - em_bits;
    if top_bits > 0 && masked_db[0] >> (8 - top_bits) != 0 {
        return false;
    }

    let db_mask = mgf1_sha256(h, db_len);
    let mut db: Vec<u8> = masked_db
        .iter()
        .zip(db_mask.iter())
        .map(|(a, b)| a ^ b)
        .collect();
    if top_bits > 0 {
        db[0] &= 0xff >> top_bits;
    }

    // DB = PS(нули) || 0x01 || salt
    let ps_len = db_len - S_LEN - 1;
    if db[..ps_len].iter().any(|&b| b != 0) || db[ps_len] != 0x01 {
        return false;
    }
    let salt = &db[ps_len + 1..];

    // M' = 00*8 || mHash || salt; H' = SHA-256(M')
    let mut m_prime = Vec::with_capacity(8 + H_LEN + S_LEN);
    m_prime.extend_from_slice(&[0u8; 8]);
    m_prime.extend_from_slice(m_hash);
    m_prime.extend_from_slice(salt);
    sha256(&m_prime)[..] == *h
}
