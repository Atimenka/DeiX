//! Беззнаковая длинная арифметика для ПРОВЕРКИ подписей RSA и ECDSA
//! (сертификаты TLS). Только операции, нужные верификатору; все входные
//! данные публичны (подписи, открытые ключи), поэтому постоянное время
//! не требуется — в отличие от x25519.rs, где обрабатывается секрет.

use alloc::vec;
use alloc::vec::Vec;
use core::cmp::Ordering;

/// Число = little-endian массив 64-битных лимбов без ведущих нулей.
#[derive(Clone, PartialEq, Eq)]
pub struct BigUint {
    limbs: Vec<u64>,
}

impl BigUint {
    pub fn zero() -> Self {
        BigUint { limbs: Vec::new() }
    }

    pub fn from_u64(v: u64) -> Self {
        let mut n = BigUint { limbs: vec![v] };
        n.normalize();
        n
    }

    pub fn from_be_bytes(b: &[u8]) -> Self {
        let mut limbs = Vec::with_capacity(b.len() / 8 + 1);
        let mut chunk_end = b.len();
        while chunk_end > 0 {
            let chunk_start = chunk_end.saturating_sub(8);
            let mut limb = 0u64;
            for &byte in &b[chunk_start..chunk_end] {
                limb = (limb << 8) | byte as u64;
            }
            limbs.push(limb);
            chunk_end = chunk_start;
        }
        let mut n = BigUint { limbs };
        n.normalize();
        n
    }

    /// Big-endian байты фиксированной длины len (слева нули).
    /// Если число длиннее len — None (подпись некорректной длины).
    pub fn to_be_bytes_padded(&self, len: usize) -> Option<Vec<u8>> {
        let mut out = vec![0u8; len];
        for (i, &limb) in self.limbs.iter().enumerate() {
            for j in 0..8 {
                let byte = ((limb >> (8 * j)) & 0xff) as u8;
                let pos = i * 8 + j;
                if pos >= len {
                    if byte != 0 {
                        return None;
                    }
                } else {
                    out[len - 1 - pos] = byte;
                }
            }
        }
        Some(out)
    }

    fn normalize(&mut self) {
        while self.limbs.last() == Some(&0) {
            self.limbs.pop();
        }
    }

    pub fn is_zero(&self) -> bool {
        self.limbs.is_empty()
    }

    pub fn bit_len(&self) -> usize {
        match self.limbs.last() {
            None => 0,
            Some(&top) => self.limbs.len() * 64 - top.leading_zeros() as usize,
        }
    }

    pub fn bit(&self, i: usize) -> bool {
        let limb = i / 64;
        if limb >= self.limbs.len() {
            return false;
        }
        (self.limbs[limb] >> (i % 64)) & 1 == 1
    }

    pub fn cmp_big(&self, other: &BigUint) -> Ordering {
        if self.limbs.len() != other.limbs.len() {
            return self.limbs.len().cmp(&other.limbs.len());
        }
        for i in (0..self.limbs.len()).rev() {
            if self.limbs[i] != other.limbs[i] {
                return self.limbs[i].cmp(&other.limbs[i]);
            }
        }
        Ordering::Equal
    }

    pub fn add_big(&self, other: &BigUint) -> BigUint {
        let n = self.limbs.len().max(other.limbs.len());
        let mut out = Vec::with_capacity(n + 1);
        let mut carry = 0u128;
        for i in 0..n {
            let a = *self.limbs.get(i).unwrap_or(&0) as u128;
            let b = *other.limbs.get(i).unwrap_or(&0) as u128;
            let s = a + b + carry;
            out.push(s as u64);
            carry = s >> 64;
        }
        if carry != 0 {
            out.push(carry as u64);
        }
        let mut r = BigUint { limbs: out };
        r.normalize();
        r
    }

    /// self - other; требует self >= other.
    pub fn sub_big(&self, other: &BigUint) -> BigUint {
        let mut out = Vec::with_capacity(self.limbs.len());
        let mut borrow = 0i128;
        for i in 0..self.limbs.len() {
            let a = self.limbs[i] as i128;
            let b = *other.limbs.get(i).unwrap_or(&0) as i128;
            let mut d = a - b - borrow;
            if d < 0 {
                d += 1i128 << 64;
                borrow = 1;
            } else {
                borrow = 0;
            }
            out.push(d as u64);
        }
        let mut r = BigUint { limbs: out };
        r.normalize();
        r
    }

    pub fn mul_big(&self, other: &BigUint) -> BigUint {
        if self.is_zero() || other.is_zero() {
            return BigUint::zero();
        }
        let mut out = vec![0u64; self.limbs.len() + other.limbs.len()];
        for (i, &a) in self.limbs.iter().enumerate() {
            let mut carry = 0u128;
            for (j, &b) in other.limbs.iter().enumerate() {
                let cur = out[i + j] as u128 + (a as u128) * (b as u128) + carry;
                out[i + j] = cur as u64;
                carry = cur >> 64;
            }
            let mut k = i + other.limbs.len();
            while carry != 0 {
                let cur = out[k] as u128 + carry;
                out[k] = cur as u64;
                carry = cur >> 64;
                k += 1;
            }
        }
        let mut r = BigUint { limbs: out };
        r.normalize();
        r
    }

    fn shl_bits(&self, bits: usize) -> BigUint {
        if self.is_zero() {
            return BigUint::zero();
        }
        let limb_shift = bits / 64;
        let bit_shift = bits % 64;
        let mut out = vec![0u64; self.limbs.len() + limb_shift + 1];
        for (i, &l) in self.limbs.iter().enumerate() {
            out[i + limb_shift] |= l << bit_shift;
            if bit_shift != 0 {
                out[i + limb_shift + 1] |= l >> (64 - bit_shift);
            }
        }
        let mut r = BigUint { limbs: out };
        r.normalize();
        r
    }

    pub fn shr1_in_place(&mut self) {
        let n = self.limbs.len();
        for i in 0..n {
            self.limbs[i] >>= 1;
            if i + 1 < n {
                self.limbs[i] |= self.limbs[i + 1] << 63;
            }
        }
        self.normalize();
    }

    /// Остаток self mod m (двоичное деление). m != 0.
    pub fn rem_big(&self, m: &BigUint) -> BigUint {
        if self.cmp_big(m) == Ordering::Less {
            return self.clone();
        }
        let shift = self.bit_len() - m.bit_len();
        let mut d = m.shl_bits(shift);
        let mut r = self.clone();
        for _ in 0..=shift {
            if r.cmp_big(&d) != Ordering::Less {
                r = r.sub_big(&d);
            }
            d.shr1_in_place();
        }
        r
    }

    pub fn mod_mul(&self, other: &BigUint, m: &BigUint) -> BigUint {
        self.mul_big(other).rem_big(m)
    }

    pub fn mod_add(&self, other: &BigUint, m: &BigUint) -> BigUint {
        self.add_big(other).rem_big(m)
    }

    /// (self - other) mod m; оба операнда уже приведены по модулю m.
    pub fn mod_sub(&self, other: &BigUint, m: &BigUint) -> BigUint {
        if self.cmp_big(other) != Ordering::Less {
            self.sub_big(other)
        } else {
            self.add_big(m).sub_big(other)
        }
    }

    /// self^exp mod m (квадрирование-умножение, старшие биты первыми).
    pub fn mod_pow(&self, exp: &BigUint, m: &BigUint) -> BigUint {
        let mut result = BigUint::from_u64(1).rem_big(m);
        let base = self.rem_big(m);
        if exp.is_zero() {
            return result;
        }
        for i in (0..exp.bit_len()).rev() {
            result = result.mod_mul(&result, m);
            if exp.bit(i) {
                result = result.mod_mul(&base, m);
            }
        }
        result
    }

    /// Обратный элемент по простому модулю m: self^(m-2) mod m
    /// (малая теорема Ферма; p и n кривых NIST — простые).
    pub fn mod_inv_prime(&self, m: &BigUint) -> BigUint {
        let two = BigUint::from_u64(2);
        self.mod_pow(&m.sub_big(&two), m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mod_pow_small() {
        // 7^560 mod 561 = 1 (561 — число Кармайкла, 7 взаимно просто)
        let b = BigUint::from_u64(7);
        let e = BigUint::from_u64(560);
        let m = BigUint::from_u64(561);
        assert_eq!(b.mod_pow(&e, &m), BigUint::from_u64(1));
    }

    #[test]
    fn test_be_bytes_roundtrip() {
        let bytes: [u8; 17] = [
            0x01, 0xff, 0x00, 0xab, 0xcd, 0xef, 0x12, 0x34, 0x56, 0x78, 0x9a,
            0xbc, 0xde, 0xf0, 0x11, 0x22, 0x33,
        ];
        let n = BigUint::from_be_bytes(&bytes);
        assert_eq!(n.to_be_bytes_padded(17).unwrap(), bytes.to_vec());
        assert_eq!(n.to_be_bytes_padded(20).unwrap()[3..], bytes[..]);
        assert!(n.to_be_bytes_padded(5).is_none());
    }

    #[test]
    fn test_mod_inv_prime() {
        // обратный к 1234 по простому модулю 10007
        let a = BigUint::from_u64(1234);
        let p = BigUint::from_u64(10007);
        let inv = a.mod_inv_prime(&p);
        assert_eq!(a.mod_mul(&inv, &p), BigUint::from_u64(1));
    }
}
