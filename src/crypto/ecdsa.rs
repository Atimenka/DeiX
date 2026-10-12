//! Проверка подписей ECDSA на кривых NIST P-256 и P-384 (FIPS 186-4,
//! SEC 1). Нужна для цепочек X.509 (ecdsa-with-SHA256/SHA384) и для
//! CertificateVerify в TLS 1.3 (ecdsa_secp256r1_sha256,
//! ecdsa_secp384r1_sha384). Только верификация: все входы публичны,
//! постоянное время не требуется.

use super::bigint::BigUint;
use alloc::vec::Vec;
use core::cmp::Ordering;

pub struct Curve {
    /// Модуль поля p.
    pub p: BigUint,
    /// Коэффициент a (для NIST-кривых p-3).
    pub a: BigUint,
    /// Коэффициент b.
    pub b: BigUint,
    /// Базовая точка G.
    pub gx: BigUint,
    pub gy: BigUint,
    /// Порядок n базовой точки (простое).
    pub n: BigUint,
    /// Размер координаты в байтах (32 для P-256, 48 для P-384).
    pub coord_len: usize,
}

const P256_P: [u8; 32] = [
    0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
];
const P256_B: [u8; 32] = [
    0x5a, 0xc6, 0x35, 0xd8, 0xaa, 0x3a, 0x93, 0xe7, 0xb3, 0xeb, 0xbd, 0x55,
    0x76, 0x98, 0x86, 0xbc, 0x65, 0x1d, 0x06, 0xb0, 0xcc, 0x53, 0xb0, 0xf6,
    0x3b, 0xce, 0x3c, 0x3e, 0x27, 0xd2, 0x60, 0x4b,
];
const P256_GX: [u8; 32] = [
    0x6b, 0x17, 0xd1, 0xf2, 0xe1, 0x2c, 0x42, 0x47, 0xf8, 0xbc, 0xe6, 0xe5,
    0x63, 0xa4, 0x40, 0xf2, 0x77, 0x03, 0x7d, 0x81, 0x2d, 0xeb, 0x33, 0xa0,
    0xf4, 0xa1, 0x39, 0x45, 0xd8, 0x98, 0xc2, 0x96,
];
const P256_GY: [u8; 32] = [
    0x4f, 0xe3, 0x42, 0xe2, 0xfe, 0x1a, 0x7f, 0x9b, 0x8e, 0xe7, 0xeb, 0x4a,
    0x7c, 0x0f, 0x9e, 0x16, 0x2b, 0xce, 0x33, 0x57, 0x6b, 0x31, 0x5e, 0xce,
    0xcb, 0xb6, 0x40, 0x68, 0x37, 0xbf, 0x51, 0xf5,
];
const P256_N: [u8; 32] = [
    0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xbc, 0xe6, 0xfa, 0xad, 0xa7, 0x17, 0x9e, 0x84,
    0xf3, 0xb9, 0xca, 0xc2, 0xfc, 0x63, 0x25, 0x51,
];

const P384_P: [u8; 48] = [
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe, 0xff, 0xff, 0xff, 0xff,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff,
];
const P384_B: [u8; 48] = [
    0xb3, 0x31, 0x2f, 0xa7, 0xe2, 0x3e, 0xe7, 0xe4, 0x98, 0x8e, 0x05, 0x6b,
    0xe3, 0xf8, 0x2d, 0x19, 0x18, 0x1d, 0x9c, 0x6e, 0xfe, 0x81, 0x41, 0x12,
    0x03, 0x14, 0x08, 0x8f, 0x50, 0x13, 0x87, 0x5a, 0xc6, 0x56, 0x39, 0x8d,
    0x8a, 0x2e, 0xd1, 0x9d, 0x2a, 0x85, 0xc8, 0xed, 0xd3, 0xec, 0x2a, 0xef,
];
const P384_GX: [u8; 48] = [
    0xaa, 0x87, 0xca, 0x22, 0xbe, 0x8b, 0x05, 0x37, 0x8e, 0xb1, 0xc7, 0x1e,
    0xf3, 0x20, 0xad, 0x74, 0x6e, 0x1d, 0x3b, 0x62, 0x8b, 0xa7, 0x9b, 0x98,
    0x59, 0xf7, 0x41, 0xe0, 0x82, 0x54, 0x2a, 0x38, 0x55, 0x02, 0xf2, 0x5d,
    0xbf, 0x55, 0x29, 0x6c, 0x3a, 0x54, 0x5e, 0x38, 0x72, 0x76, 0x0a, 0xb7,
];
const P384_GY: [u8; 48] = [
    0x36, 0x17, 0xde, 0x4a, 0x96, 0x26, 0x2c, 0x6f, 0x5d, 0x9e, 0x98, 0xbf,
    0x92, 0x92, 0xdc, 0x29, 0xf8, 0xf4, 0x1d, 0xbd, 0x28, 0x9a, 0x14, 0x7c,
    0xe9, 0xda, 0x31, 0x13, 0xb5, 0xf0, 0xb8, 0xc0, 0x0a, 0x60, 0xb1, 0xce,
    0x1d, 0x7e, 0x81, 0x9d, 0x7a, 0x43, 0x1d, 0x7c, 0x90, 0xea, 0x0e, 0x5f,
];
const P384_N: [u8; 48] = [
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xc7, 0x63, 0x4d, 0x81, 0xf4, 0x37, 0x2d, 0xdf, 0x58, 0x1a, 0x0d, 0xb2,
    0x48, 0xb0, 0xa7, 0x7a, 0xec, 0xec, 0x19, 0x6a, 0xcc, 0xc5, 0x29, 0x73,
];

fn three() -> BigUint {
    BigUint::from_u64(3)
}

pub fn p256() -> Curve {
    let p = BigUint::from_be_bytes(&P256_P);
    let a = p.sub_big(&three());
    Curve {
        a,
        b: BigUint::from_be_bytes(&P256_B),
        gx: BigUint::from_be_bytes(&P256_GX),
        gy: BigUint::from_be_bytes(&P256_GY),
        n: BigUint::from_be_bytes(&P256_N),
        coord_len: 32,
        p,
    }
}

pub fn p384() -> Curve {
    let p = BigUint::from_be_bytes(&P384_P);
    let a = p.sub_big(&three());
    Curve {
        a,
        b: BigUint::from_be_bytes(&P384_B),
        gx: BigUint::from_be_bytes(&P384_GX),
        gy: BigUint::from_be_bytes(&P384_GY),
        n: BigUint::from_be_bytes(&P384_N),
        coord_len: 48,
        p,
    }
}

/// Точка в якобиановых координатах; z=0 — бесконечность.
#[derive(Clone)]
struct JPoint {
    x: BigUint,
    y: BigUint,
    z: BigUint,
}

impl JPoint {
    fn infinity() -> Self {
        JPoint {
            x: BigUint::from_u64(1),
            y: BigUint::from_u64(1),
            z: BigUint::zero(),
        }
    }

    fn from_affine(x: &BigUint, y: &BigUint) -> Self {
        JPoint {
            x: x.clone(),
            y: y.clone(),
            z: BigUint::from_u64(1),
        }
    }

    fn is_infinity(&self) -> bool {
        self.z.is_zero()
    }
}

/// Удвоение точки (формулы dbl-2007-bl без оптимизации a=-3,
/// работает для произвольного a).
fn point_double(c: &Curve, pt: &JPoint) -> JPoint {
    if pt.is_infinity() || pt.y.is_zero() {
        return JPoint::infinity();
    }
    let p = &c.p;
    let xx = pt.x.mod_mul(&pt.x, p);
    let yy = pt.y.mod_mul(&pt.y, p);
    let yyyy = yy.mod_mul(&yy, p);
    let zz = pt.z.mod_mul(&pt.z, p);
    // S = 4*X*YY
    let s = BigUint::from_u64(4).mod_mul(&pt.x.mod_mul(&yy, p), p);
    // M = 3*XX + a*ZZ^2
    let m = three()
        .mod_mul(&xx, p)
        .mod_add(&c.a.mod_mul(&zz.mod_mul(&zz, p), p), p);
    // X' = M^2 - 2*S
    let x3 = m.mod_mul(&m, p).mod_sub(&s.mod_add(&s, p), p);
    // Y' = M*(S - X') - 8*YYYY
    let y3 = m
        .mod_mul(&s.mod_sub(&x3, p), p)
        .mod_sub(&BigUint::from_u64(8).mod_mul(&yyyy, p), p);
    // Z' = 2*Y*Z
    let z3 = BigUint::from_u64(2).mod_mul(&pt.y.mod_mul(&pt.z, p), p);
    JPoint { x: x3, y: y3, z: z3 }
}

/// Сложение точек в якобиановых координатах (add-2007-bl).
fn point_add(c: &Curve, a: &JPoint, b: &JPoint) -> JPoint {
    if a.is_infinity() {
        return b.clone();
    }
    if b.is_infinity() {
        return a.clone();
    }
    let p = &c.p;
    let z1z1 = a.z.mod_mul(&a.z, p);
    let z2z2 = b.z.mod_mul(&b.z, p);
    let u1 = a.x.mod_mul(&z2z2, p);
    let u2 = b.x.mod_mul(&z1z1, p);
    let s1 = a.y.mod_mul(&b.z.mod_mul(&z2z2, p), p);
    let s2 = b.y.mod_mul(&a.z.mod_mul(&z1z1, p), p);
    if u1 == u2 {
        if s1 == s2 {
            return point_double(c, a);
        }
        return JPoint::infinity(); // P + (-P)
    }
    let h = u2.mod_sub(&u1, p);
    let r = s2.mod_sub(&s1, p);
    let hh = h.mod_mul(&h, p);
    let hhh = hh.mod_mul(&h, p);
    let v = u1.mod_mul(&hh, p);
    // X3 = R^2 - HHH - 2*V
    let x3 = r
        .mod_mul(&r, p)
        .mod_sub(&hhh, p)
        .mod_sub(&v.mod_add(&v, p), p);
    // Y3 = R*(V - X3) - S1*HHH
    let y3 = r
        .mod_mul(&v.mod_sub(&x3, p), p)
        .mod_sub(&s1.mod_mul(&hhh, p), p);
    // Z3 = Z1*Z2*H
    let z3 = a.z.mod_mul(&b.z.mod_mul(&h, p), p);
    JPoint { x: x3, y: y3, z: z3 }
}

/// Скалярное умножение (двоичный метод, старшие биты первыми).
fn scalar_mul(c: &Curve, k: &BigUint, pt: &JPoint) -> JPoint {
    let mut acc = JPoint::infinity();
    for i in (0..k.bit_len()).rev() {
        acc = point_double(c, &acc);
        if k.bit(i) {
            acc = point_add(c, &acc, pt);
        }
    }
    acc
}

/// Якобиановы -> аффинные координаты.
fn to_affine(c: &Curve, pt: &JPoint) -> Option<(BigUint, BigUint)> {
    if pt.is_infinity() {
        return None;
    }
    let zinv = pt.z.mod_inv_prime(&c.p);
    let zinv2 = zinv.mod_mul(&zinv, &c.p);
    let x = pt.x.mod_mul(&zinv2, &c.p);
    let y = pt.y.mod_mul(&zinv2.mod_mul(&zinv, &c.p), &c.p);
    Some((x, y))
}

/// Точка лежит на кривой: y^2 = x^3 + a*x + b (mod p).
fn on_curve(c: &Curve, x: &BigUint, y: &BigUint) -> bool {
    if x.cmp_big(&c.p) != Ordering::Less || y.cmp_big(&c.p) != Ordering::Less {
        return false;
    }
    let lhs = y.mod_mul(y, &c.p);
    let rhs = x
        .mod_mul(&x.mod_mul(x, &c.p), &c.p)
        .mod_add(&c.a.mod_mul(x, &c.p), &c.p)
        .mod_add(&c.b, &c.p);
    lhs == rhs
}

/// Хэш -> целое по SEC 1 §4.1.4: левые bit_len(n) бит.
fn hash_to_int(c: &Curve, hash: &[u8]) -> BigUint {
    let n_bits = c.n.bit_len();
    let mut e = BigUint::from_be_bytes(hash);
    let hash_bits = hash.len() * 8;
    if hash_bits > n_bits {
        for _ in 0..(hash_bits - n_bits) {
            e.shr1_in_place();
        }
    }
    e
}

/// Проверка подписи ECDSA (SEC 1 §4.1.4).
/// q — открытый ключ в несжатом виде SEC1: 0x04 || X || Y.
/// r, s — компоненты подписи (big-endian), hash — хэш сообщения.
pub fn verify(c: &Curve, q: &[u8], hash: &[u8], r: &[u8], s: &[u8]) -> bool {
    // ключ: несжатая точка
    if q.len() != 1 + 2 * c.coord_len || q[0] != 0x04 {
        return false;
    }
    let qx = BigUint::from_be_bytes(&q[1..1 + c.coord_len]);
    let qy = BigUint::from_be_bytes(&q[1 + c.coord_len..]);
    if !on_curve(c, &qx, &qy) {
        return false;
    }

    let r_int = BigUint::from_be_bytes(r);
    let s_int = BigUint::from_be_bytes(s);
    let one = BigUint::from_u64(1);
    if r_int.cmp_big(&one) == Ordering::Less || r_int.cmp_big(&c.n) != Ordering::Less {
        return false;
    }
    if s_int.cmp_big(&one) == Ordering::Less || s_int.cmp_big(&c.n) != Ordering::Less {
        return false;
    }

    let e = hash_to_int(c, hash).rem_big(&c.n);
    let w = s_int.mod_inv_prime(&c.n);
    let u1 = e.mod_mul(&w, &c.n);
    let u2 = r_int.mod_mul(&w, &c.n);

    let g = JPoint::from_affine(&c.gx, &c.gy);
    let qpt = JPoint::from_affine(&qx, &qy);
    let sum = point_add(c, &scalar_mul(c, &u1, &g), &scalar_mul(c, &u2, &qpt));
    match to_affine(c, &sum) {
        Some((x, _)) => x.rem_big(&c.n) == r_int,
        None => false,
    }
}
