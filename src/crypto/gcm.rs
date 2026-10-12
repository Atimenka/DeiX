//! AES-128-GCM (NIST SP 800-38D) — AEAD-режим для TLS 1.3.
//!
//! GHASH реализован побитово (алгоритм 1 из SP 800-38D), без таблиц:
//! медленнее табличных вариантов, но компактно и без побочных каналов
//! через кэш. Для страниц в сотни килобайт на эмуляторе этого достаточно.

use super::aes::Aes128;
use alloc::vec::Vec;

pub const TAG_LEN: usize = 16;

pub struct AesGcm128 {
    aes: Aes128,
    /// H = E(K, 0^128) — ключ GHASH.
    h: [u8; 16],
}

/// Умножение в GF(2^128) с «отражённым» полиномом R = 0xE1·x^120
/// (SP 800-38D, алгоритм 1): z = x · y.
fn gf_mul(x: &[u8; 16], y: &[u8; 16]) -> [u8; 16] {
    let mut z = [0u8; 16];
    let mut v = *y;
    for i in 0..128 {
        if (x[i / 8] >> (7 - (i % 8))) & 1 == 1 {
            for k in 0..16 {
                z[k] ^= v[k];
            }
        }
        let lsb = v[15] & 1;
        let mut carry = 0u8;
        for k in 0..16 {
            let next_carry = v[k] & 1;
            v[k] = (v[k] >> 1) | (carry << 7);
            carry = next_carry;
        }
        if lsb == 1 {
            v[0] ^= 0xE1;
        }
    }
    z
}

impl AesGcm128 {
    pub fn new(key: &[u8; 16]) -> Self {
        let aes = Aes128::new(key);
        let mut h = [0u8; 16];
        aes.encrypt_block(&mut h);
        AesGcm128 { aes, h }
    }

    /// GHASH по aad и ciphertext с длинами (в битах) в финальном блоке.
    fn ghash(&self, aad: &[u8], ct: &[u8]) -> [u8; 16] {
        let mut y = [0u8; 16];
        let absorb = |data: &[u8], y: &mut [u8; 16]| {
            for chunk in data.chunks(16) {
                let mut block = [0u8; 16];
                block[..chunk.len()].copy_from_slice(chunk);
                for k in 0..16 {
                    y[k] ^= block[k];
                }
                *y = gf_mul(y, &self.h);
            }
        };
        absorb(aad, &mut y);
        absorb(ct, &mut y);
        let mut lens = [0u8; 16];
        lens[..8].copy_from_slice(&((aad.len() as u64) * 8).to_be_bytes());
        lens[8..].copy_from_slice(&((ct.len() as u64) * 8).to_be_bytes());
        for k in 0..16 {
            y[k] ^= lens[k];
        }
        gf_mul(&y, &self.h)
    }

    /// Счётчиковый блок J0 для 96-битного nonce: nonce || 0x00000001.
    fn j0(nonce: &[u8; 12]) -> [u8; 16] {
        let mut j = [0u8; 16];
        j[..12].copy_from_slice(nonce);
        j[15] = 1;
        j
    }

    fn inc32(block: &mut [u8; 16]) {
        let mut ctr = u32::from_be_bytes([block[12], block[13], block[14], block[15]]);
        ctr = ctr.wrapping_add(1);
        block[12..].copy_from_slice(&ctr.to_be_bytes());
    }

    /// CTR-шифрование начиная с inc32(J0) — одинаково для seal и open.
    fn ctr_xor(&self, nonce: &[u8; 12], data: &mut [u8]) {
        let mut counter = Self::j0(nonce);
        for chunk in data.chunks_mut(16) {
            Self::inc32(&mut counter);
            let mut keystream = counter;
            self.aes.encrypt_block(&mut keystream);
            for (b, k) in chunk.iter_mut().zip(keystream.iter()) {
                *b ^= k;
            }
        }
    }

    fn tag(&self, nonce: &[u8; 12], aad: &[u8], ct: &[u8]) -> [u8; 16] {
        let mut t = self.ghash(aad, ct);
        let mut ekj0 = Self::j0(nonce);
        self.aes.encrypt_block(&mut ekj0);
        for k in 0..16 {
            t[k] ^= ekj0[k];
        }
        t
    }

    /// Шифрует и аутентифицирует: возвращает ciphertext || tag(16).
    pub fn seal(&self, nonce: &[u8; 12], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(plaintext.len() + TAG_LEN);
        out.extend_from_slice(plaintext);
        self.ctr_xor(nonce, &mut out);
        let tag = self.tag(nonce, aad, &out);
        out.extend_from_slice(&tag);
        out
    }

    /// Проверяет тег и расшифровывает. None — подделка/повреждение.
    pub fn open(&self, nonce: &[u8; 12], aad: &[u8], ct_with_tag: &[u8]) -> Option<Vec<u8>> {
        if ct_with_tag.len() < TAG_LEN {
            return None;
        }
        let (ct, tag) = ct_with_tag.split_at(ct_with_tag.len() - TAG_LEN);
        let expect = self.tag(nonce, aad, ct);
        // сравнение без ранних выходов
        let mut diff = 0u8;
        for k in 0..TAG_LEN {
            diff |= expect[k] ^ tag[k];
        }
        if diff != 0 {
            return None;
        }
        let mut out = Vec::from(ct);
        self.ctr_xor(nonce, &mut out);
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gcm_nist_case3() {
        // NIST GCM test case 3 (AES-128, 96-битный IV, без AAD).
        let key: [u8; 16] = [
            0xfe, 0xff, 0xe9, 0x92, 0x86, 0x65, 0x73, 0x1c,
            0x6d, 0x6a, 0x8f, 0x94, 0x67, 0x30, 0x83, 0x08,
        ];
        let nonce: [u8; 12] = [
            0xca, 0xfe, 0xba, 0xbe, 0xfa, 0xce, 0xdb, 0xad, 0xde, 0xca, 0xf8, 0x88,
        ];
        let plaintext: [u8; 64] = [
            0xd9, 0x31, 0x32, 0x25, 0xf8, 0x84, 0x06, 0xe5,
            0xa5, 0x59, 0x09, 0xc5, 0xaf, 0xf5, 0x26, 0x9a,
            0x86, 0xa7, 0xa9, 0x53, 0x15, 0x34, 0xf7, 0xda,
            0x2e, 0x4c, 0x30, 0x3d, 0x8a, 0x31, 0x8a, 0x72,
            0x1c, 0x3c, 0x0c, 0x95, 0x95, 0x68, 0x09, 0x53,
            0x2f, 0xcf, 0x0e, 0x24, 0x49, 0xa6, 0xb5, 0x25,
            0xb1, 0x6a, 0xed, 0xf5, 0xaa, 0x0d, 0xe6, 0x57,
            0xba, 0x63, 0x7b, 0x39, 0x1a, 0xaf, 0xd2, 0x55,
        ];
        let gcm = AesGcm128::new(&key);
        let sealed = gcm.seal(&nonce, &[], &plaintext);
        assert_eq!(sealed.len(), 64 + TAG_LEN);
        // Первые байты шифртекста из NIST-вектора.
        assert_eq!(&sealed[..8], &[0x42, 0x83, 0x1e, 0xc2, 0x21, 0x77, 0x74, 0x24]);
        // Тег из NIST-вектора.
        assert_eq!(
            &sealed[64..],
            &[
                0x4d, 0x5c, 0x2a, 0xf3, 0x27, 0xcd, 0x64, 0xa6,
                0x2c, 0xf3, 0x5a, 0xbd, 0x2b, 0xa6, 0xfa, 0xb4,
            ]
        );
        let opened = gcm.open(&nonce, &[], &sealed).expect("tag must verify");
        assert_eq!(&opened[..], &plaintext[..]);
    }
}
