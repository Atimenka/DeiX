//! X25519 (RFC 7748) — обмен ключами Диффи-Хеллмана на Curve25519.
//!
//! Порт референсной реализации TweetNaCl (public domain): поле
//! GF(2^255-19) в 16 лимбах по 16 бит (i64), лестница Монтгомери с
//! постоянным временем (условные обмены sel25519 без ветвлений по
//! секретным данным). Используется TLS 1.3 для key_share.

type Gf = [i64; 16];

const K121665: Gf = [0xDB41, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

/// Базовая точка Curve25519: u = 9.
pub const BASEPOINT: [u8; 32] = [
    9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

fn car25519(o: &mut Gf) {
    for i in 0..16 {
        o[i] += 1 << 16;
        let c = o[i] >> 16;
        if i < 15 {
            o[i + 1] += c - 1;
        } else {
            o[0] += 38 * (c - 1);
        }
        o[i] -= c << 16;
    }
}

/// Условный обмен p<->q при b=1 (без ветвления по b).
fn sel25519(p: &mut Gf, q: &mut Gf, b: i64) {
    let c = !(b - 1);
    for i in 0..16 {
        let t = c & (p[i] ^ q[i]);
        p[i] ^= t;
        q[i] ^= t;
    }
}

fn pack25519(o: &mut [u8; 32], n: &Gf) {
    let mut t = *n;
    car25519(&mut t);
    car25519(&mut t);
    car25519(&mut t);
    for _ in 0..2 {
        let mut m: Gf = [0; 16];
        m[0] = t[0] - 0xffed;
        for i in 1..15 {
            m[i] = t[i] - 0xffff - ((m[i - 1] >> 16) & 1);
            m[i - 1] &= 0xffff;
        }
        m[15] = t[15] - 0x7fff - ((m[14] >> 16) & 1);
        let b = (m[15] >> 16) & 1;
        m[14] &= 0xffff;
        sel25519(&mut t, &mut m, 1 - b);
    }
    for i in 0..16 {
        o[2 * i] = (t[i] & 0xff) as u8;
        o[2 * i + 1] = ((t[i] >> 8) & 0xff) as u8;
    }
}

fn unpack25519(n: &[u8; 32]) -> Gf {
    let mut o: Gf = [0; 16];
    for i in 0..16 {
        o[i] = n[2 * i] as i64 + ((n[2 * i + 1] as i64) << 8);
    }
    o[15] &= 0x7fff;
    o
}

fn add(a: &Gf, b: &Gf) -> Gf {
    let mut o: Gf = [0; 16];
    for i in 0..16 {
        o[i] = a[i] + b[i];
    }
    o
}

fn sub(a: &Gf, b: &Gf) -> Gf {
    let mut o: Gf = [0; 16];
    for i in 0..16 {
        o[i] = a[i] - b[i];
    }
    o
}

fn mul(a: &Gf, b: &Gf) -> Gf {
    let mut t = [0i64; 31];
    for i in 0..16 {
        for j in 0..16 {
            t[i + j] += a[i] * b[j];
        }
    }
    for i in 0..15 {
        t[i] += 38 * t[i + 16];
    }
    let mut o: Gf = [0; 16];
    o.copy_from_slice(&t[..16]);
    car25519(&mut o);
    car25519(&mut o);
    o
}

/// Обращение в поле: a^(p-2), цепочка возведений как в TweetNaCl.
fn inv25519(i: &Gf) -> Gf {
    let mut c = *i;
    for a in (0..=253).rev() {
        c = mul(&c, &c);
        if a != 2 && a != 4 {
            c = mul(&c, i);
        }
    }
    c
}

/// Скалярное умножение X25519: q = n * p (RFC 7748 §5).
pub fn x25519(n: &[u8; 32], p: &[u8; 32]) -> [u8; 32] {
    let mut z = *n;
    z[31] = (n[31] & 127) | 64;
    z[0] &= 248;

    let x = unpack25519(p);
    let mut a: Gf = [0; 16];
    let mut b = x;
    let mut c: Gf = [0; 16];
    let mut d: Gf = [0; 16];
    a[0] = 1;
    d[0] = 1;

    for i in (0..=254usize).rev() {
        let r = ((z[i >> 3] >> (i & 7)) & 1) as i64;
        sel25519(&mut a, &mut b, r);
        sel25519(&mut c, &mut d, r);

        let e = add(&a, &c);
        let a1 = sub(&a, &c);
        let c1 = add(&b, &d);
        let b1 = sub(&b, &d);
        d = mul(&e, &e);
        let f = mul(&a1, &a1);
        let a2 = mul(&c1, &a1);
        let c2 = mul(&b1, &e);
        let e1 = add(&a2, &c2);
        let a3 = sub(&a2, &c2);
        b = mul(&a3, &a3);
        let c3 = sub(&d, &f);
        let a4 = mul(&c3, &K121665);
        let a5 = add(&a4, &d);
        c = mul(&c3, &a5);
        a = mul(&d, &f);
        d = mul(&b, &x);
        b = mul(&e1, &e1);

        sel25519(&mut a, &mut b, r);
        sel25519(&mut c, &mut d, r);
    }

    let c_inv = inv25519(&c);
    let out = mul(&a, &c_inv);
    let mut q = [0u8; 32];
    pack25519(&mut q, &out);
    q
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rfc7748_vector1() {
        // RFC 7748 §5.2, первый тест-вектор.
        let scalar: [u8; 32] = [
            0xa5, 0x46, 0xe3, 0x6b, 0xf0, 0x52, 0x7c, 0x9d,
            0x3b, 0x16, 0x15, 0x4b, 0x82, 0x46, 0x5e, 0xdd,
            0x62, 0x14, 0x4c, 0x0a, 0xc1, 0xfc, 0x5a, 0x18,
            0x50, 0x6a, 0x22, 0x44, 0xba, 0x44, 0x9a, 0xc4,
        ];
        let point: [u8; 32] = [
            0xe6, 0xdb, 0x68, 0x67, 0x58, 0x30, 0x30, 0xdb,
            0x35, 0x94, 0xc1, 0xa4, 0x24, 0xb1, 0x5f, 0x7c,
            0x72, 0x66, 0x24, 0xec, 0x26, 0xb3, 0x35, 0x3b,
            0x10, 0xa9, 0x03, 0xa6, 0xd0, 0xab, 0x1c, 0x4c,
        ];
        let expect: [u8; 32] = [
            0xc3, 0xda, 0x55, 0x37, 0x9d, 0xe9, 0xc6, 0x90,
            0x8e, 0x94, 0xea, 0x4d, 0xf2, 0x8d, 0x08, 0x4f,
            0x32, 0xec, 0xcf, 0x03, 0x49, 0x1c, 0x71, 0xf7,
            0x54, 0xb4, 0x07, 0x55, 0x77, 0xa2, 0x85, 0x52,
        ];
        assert_eq!(x25519(&scalar, &point), expect);
    }

    #[test]
    fn test_dh_agreement() {
        // RFC 7748 §6.1: разделяемый секрет Алисы и Боба совпадает.
        let alice_priv: [u8; 32] = [
            0x77, 0x07, 0x6d, 0x0a, 0x73, 0x18, 0xa5, 0x7d,
            0x3c, 0x16, 0xc1, 0x72, 0x51, 0xb2, 0x66, 0x45,
            0xdf, 0x4c, 0x2f, 0x87, 0xeb, 0xc0, 0x99, 0x2a,
            0xb1, 0x77, 0xfb, 0xa5, 0x1d, 0xb9, 0x2c, 0x2a,
        ];
        let bob_priv: [u8; 32] = [
            0x5d, 0xab, 0x08, 0x7e, 0x62, 0x4a, 0x8a, 0x4b,
            0x79, 0xe1, 0x7f, 0x8b, 0x83, 0x80, 0x0e, 0xe6,
            0x6f, 0x3b, 0xb1, 0x29, 0x26, 0x18, 0xb6, 0xfd,
            0x1c, 0x2f, 0x8b, 0x27, 0xff, 0x88, 0xe0, 0xeb,
        ];
        let alice_pub = x25519(&alice_priv, &BASEPOINT);
        let bob_pub = x25519(&bob_priv, &BASEPOINT);
        assert_eq!(
            x25519(&alice_priv, &bob_pub),
            x25519(&bob_priv, &alice_pub)
        );
    }
}
