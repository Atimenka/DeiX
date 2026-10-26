//! SHA-512 (FIPS 180-4) — реализация с нуля, без внешних crate (та же
//! методология, что и sha256.rs). Используется в crypto_storage.rs для
//! получения 512-битного ключа шифрования диска из пароля — это
//! ТОЧНО СООТВЕТСТВУЕТ схеме, которую использует настоящий Linux
//! dm-crypt/cryptsetup в режиме `--type plain --cipher aes-xts-plain64
//! --key-size 512 --hash sha512`: пароль хэшируется SHA-512 и полученные
//! 64 байта используются напрямую как двоичный XTS-ключ (никакого
//! PBKDF2/соли в plain-режиме — так работает именно "plain" dm-crypt,
//! не LUKS; см. подробное объяснение в module-level документации
//! crypto_storage.rs).
//!
//! Проверено по официальным тестовым векторам NIST (SHA-512("") и
//! SHA-512("abc")) и сверено с эталонной реализацией Python
//! `hashlib.sha512`.

pub const DIGEST_LEN: usize = 64;

const H0: [u64; 8] = [
    0x6a09e667f3bcc908, 0xbb67ae8584caa73b, 0x3c6ef372fe94f82b, 0xa54ff53a5f1d36f1,
    0x510e527fade682d1, 0x9b05688c2b3e6c1f, 0x1f83d9abfb41bd6b, 0x5be0cd19137e2179,
];

const K: [u64; 80] = [
    0x428a2f98d728ae22, 0x7137449123ef65cd, 0xb5c0fbcfec4d3b2f, 0xe9b5dba58189dbbc,
    0x3956c25bf348b538, 0x59f111f1b605d019, 0x923f82a4af194f9b, 0xab1c5ed5da6d8118,
    0xd807aa98a3030242, 0x12835b0145706fbe, 0x243185be4ee4b28c, 0x550c7dc3d5ffb4e2,
    0x72be5d74f27b896f, 0x80deb1fe3b1696b1, 0x9bdc06a725c71235, 0xc19bf174cf692694,
    0xe49b69c19ef14ad2, 0xefbe4786384f25e3, 0x0fc19dc68b8cd5b5, 0x240ca1cc77ac9c65,
    0x2de92c6f592b0275, 0x4a7484aa6ea6e483, 0x5cb0a9dcbd41fbd4, 0x76f988da831153b5,
    0x983e5152ee66dfab, 0xa831c66d2db43210, 0xb00327c898fb213f, 0xbf597fc7beef0ee4,
    0xc6e00bf33da88fc2, 0xd5a79147930aa725, 0x06ca6351e003826f, 0x142929670a0e6e70,
    0x27b70a8546d22ffc, 0x2e1b21385c26c926, 0x4d2c6dfc5ac42aed, 0x53380d139d95b3df,
    0x650a73548baf63de, 0x766a0abb3c77b2a8, 0x81c2c92e47edaee6, 0x92722c851482353b,
    0xa2bfe8a14cf10364, 0xa81a664bbc423001, 0xc24b8b70d0f89791, 0xc76c51a30654be30,
    0xd192e819d6ef5218, 0xd69906245565a910, 0xf40e35855771202a, 0x106aa07032bbd1b8,
    0x19a4c116b8d2d0c8, 0x1e376c085141ab53, 0x2748774cdf8eeb99, 0x34b0bcb5e19b48a8,
    0x391c0cb3c5c95a63, 0x4ed8aa4ae3418acb, 0x5b9cca4f7763e373, 0x682e6ff3d6b2b8a3,
    0x748f82ee5defb2fc, 0x78a5636f43172f60, 0x84c87814a1f0ab72, 0x8cc702081a6439ec,
    0x90befffa23631e28, 0xa4506cebde82bde9, 0xbef9a3f7b2c67915, 0xc67178f2e372532b,
    0xca273eceea26619c, 0xd186b8c721c0c207, 0xeada7dd6cde0eb1e, 0xf57d4f7fee6ed178,
    0x06f067aa72176fba, 0x0a637dc5a2c898a6, 0x113f9804bef90dae, 0x1b710b35131c471b,
    0x28db77f523047d84, 0x32caab7b40c72493, 0x3c9ebe0a15c9bebc, 0x431d67c49c100d4c,
    0x4cc5d4becb3e42b6, 0x597f299cfc657e2a, 0x5fcb6fab3ad6faec, 0x6c44198c4a475817,
];

/// Один раунд сжатия без пересылки регистров: вместо сдвига a..h на каждом
/// раунде имена переменных вращаются между восемью раундами подряд.
/// Результат побитово тот же, что у «классической» записи со сдвигом.
macro_rules! round {
    ($a:ident, $b:ident, $c:ident, $d:ident, $e:ident, $f:ident, $g:ident, $h:ident, $k:expr, $w:expr) => {
        let t1 = $h
            .wrapping_add($e.rotate_right(14) ^ $e.rotate_right(18) ^ $e.rotate_right(41))
            .wrapping_add($g ^ ($e & ($f ^ $g)))
            .wrapping_add($k)
            .wrapping_add($w);
        let t2 = ($a.rotate_right(28) ^ $a.rotate_right(34) ^ $a.rotate_right(39))
            .wrapping_add(($a & $b) | ($c & ($a | $b)));
        $d = $d.wrapping_add(t1);
        $h = t1.wrapping_add(t2);
    };
}

#[derive(Clone)]
pub struct Sha512 {
    state: [u64; 8],
    buffer: [u8; 128],
    buffer_len: usize,
    total_len: u128,
}

impl Sha512 {
    pub fn new() -> Self {
        Sha512 {
            state: H0,
            buffer: [0u8; 128],
            buffer_len: 0,
            total_len: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.total_len += data.len() as u128;

        if self.buffer_len > 0 {
            let need = 128 - self.buffer_len;
            let take = need.min(data.len());
            self.buffer[self.buffer_len..self.buffer_len + take].copy_from_slice(&data[..take]);
            self.buffer_len += take;
            data = &data[take..];

            if self.buffer_len == 128 {
                let block = self.buffer;
                self.process_block(&block);
                self.buffer_len = 0;
            }
        }

        while data.len() >= 128 {
            let mut block = [0u8; 128];
            block.copy_from_slice(&data[..128]);
            self.process_block(&block);
            data = &data[128..];
        }

        if !data.is_empty() {
            self.buffer[..data.len()].copy_from_slice(data);
            self.buffer_len = data.len();
        }
    }

    pub fn finalize(mut self) -> [u8; DIGEST_LEN] {
        let bit_len = self.total_len * 8;

        let mut at = self.buffer_len;
        self.buffer[at] = 0x80;
        at += 1;
        if at > 112 {
            self.buffer[at..].fill(0);
            let block = self.buffer;
            self.process_block(&block);
            at = 0;
        }
        self.buffer[at..112].fill(0);
        // 128-битная длина в битах, big-endian (SHA-512 использует
        // 128-битное поле длины, а не 64-битное, как SHA-256).
        self.buffer[112..].copy_from_slice(&bit_len.to_be_bytes());
        let block = self.buffer;
        self.process_block(&block);

        let mut out = [0u8; DIGEST_LEN];
        for (i, word) in self.state.iter().enumerate() {
            out[i * 8..i * 8 + 8].copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    /// Затирает внутреннее состояние через write_volatile: в нём лежит
    /// материал, производный от ключа (HMAC-состояния PBKDF2).
    pub fn wipe(&mut self) {
        for word in self.state.iter_mut() {
            unsafe { core::ptr::write_volatile(word, 0) };
        }
        for byte in self.buffer.iter_mut() {
            unsafe { core::ptr::write_volatile(byte, 0) };
        }
    }

    fn process_block(&mut self, block: &[u8; 128]) {
        let mut w = [0u64; 16];
        for (i, word) in w.iter_mut().enumerate() {
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&block[i * 8..i * 8 + 8]);
            *word = u64::from_be_bytes(bytes);
        }
        compress(&mut self.state, &w);
    }

    /// Состояние после этого хеша и ещё одного сообщения ровно в 64 байта,
    /// заданного восемью словами: то же, что `clone` + `update` + `finalize`,
    /// но без буфера и перевода слов в байты. Нужен PBKDF2, где так хешируется
    /// каждая из сотен тысяч итераций. Состояние должно стоять на границе
    /// блока (после целого числа блоков).
    pub(crate) fn digest_of_words(&self, message: &[u64; 8]) -> [u64; 8] {
        debug_assert_eq!(self.buffer_len, 0);
        let mut w = [0u64; 16];
        w[..8].copy_from_slice(message);
        w[8] = 0x8000_0000_0000_0000;
        w[15] = (self.total_len as u64 + 64) * 8;
        let mut state = self.state;
        compress(&mut state, &w);
        state
    }
}

/// Сжатие одного блока, заданного первыми 16 словами расписания.
fn compress(state: &mut [u64; 8], block: &[u64; 16]) {
    let mut w = [0u64; 80];
    w[..16].copy_from_slice(block);
    for i in 16..80 {
        let s0 = w[i - 15].rotate_right(1) ^ w[i - 15].rotate_right(8) ^ (w[i - 15] >> 7);
        let s1 = w[i - 2].rotate_right(19) ^ w[i - 2].rotate_right(61) ^ (w[i - 2] >> 6);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }

    let mut a = state[0];
    let mut b = state[1];
    let mut c = state[2];
    let mut d = state[3];
    let mut e = state[4];
    let mut f = state[5];
    let mut g = state[6];
    let mut h = state[7];

    for chunk in 0..10 {
        let i = chunk * 8;
        round!(a, b, c, d, e, f, g, h, K[i], w[i]);
        round!(h, a, b, c, d, e, f, g, K[i + 1], w[i + 1]);
        round!(g, h, a, b, c, d, e, f, K[i + 2], w[i + 2]);
        round!(f, g, h, a, b, c, d, e, K[i + 3], w[i + 3]);
        round!(e, f, g, h, a, b, c, d, K[i + 4], w[i + 4]);
        round!(d, e, f, g, h, a, b, c, K[i + 5], w[i + 5]);
        round!(c, d, e, f, g, h, a, b, K[i + 6], w[i + 6]);
        round!(b, c, d, e, f, g, h, a, K[i + 7], w[i + 7]);
    }

    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
    state[5] = state[5].wrapping_add(f);
    state[6] = state[6].wrapping_add(g);
    state[7] = state[7].wrapping_add(h);
}

pub fn sha512(data: &[u8]) -> [u8; DIGEST_LEN] {
    let mut h = Sha512::new();
    h.update(data);
    h.finalize()
}

/// Начальное состояние SHA-384 (FIPS 180-4 §5.3.4).
const H0_384: [u64; 8] = [
    0xcbbb9d5dc1059ed8, 0x629a292a367cd507, 0x9159015a3070dd17, 0x152fecd8f70e5939,
    0x67332667ffc00b31, 0x8eb44a8768581511, 0xdb0c2e0d64f98fa7, 0x47b5481dbefa4fa4,
];

/// SHA-384 (FIPS 180-4): тот же конвейер, что SHA-512, но с другим
/// начальным состоянием и усечением до 48 байт. Нужен для проверки
/// подписей сертификатов sha384WithRSA / ecdsa-with-SHA384.
pub fn sha384(data: &[u8]) -> [u8; 48] {
    let mut h = Sha512::new();
    h.state = H0_384;
    h.update(data);
    let full = h.finalize();
    let mut out = [0u8; 48];
    out.copy_from_slice(&full[..48]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_hex(bytes: &[u8]) -> alloc::string::String {
        use alloc::string::String;
        use core::fmt::Write;
        let mut s = String::new();
        for b in bytes {
            write!(s, "{:02x}", b).unwrap();
        }
        s
    }

    #[test]
    fn test_digest_of_words_matches_update() {
        let mut base = Sha512::new();
        base.update(&[0x36u8; 128]);
        let words = [1u64, 2, 3, 4, 5, 6, 7, 0xffff_ffff_ffff_fffe];
        let mut bytes = [0u8; 64];
        for (i, word) in words.iter().enumerate() {
            bytes[i * 8..i * 8 + 8].copy_from_slice(&word.to_be_bytes());
        }
        let mut full = base.clone();
        full.update(&bytes);
        let expected = full.finalize();
        let got = base.digest_of_words(&words);
        for (i, word) in got.iter().enumerate() {
            assert_eq!(word.to_be_bytes(), expected[i * 8..i * 8 + 8]);
        }
    }

    #[test]
    fn test_sha384_abc() {
        // Официальный вектор NIST: SHA-384("abc").
        let d = sha384(b"abc");
        assert_eq!(
            to_hex(&d),
            "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded163\
1a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7"
        );
    }

    #[test]
    fn test_empty_string() {
        let digest = sha512(b"");
        assert_eq!(
            to_hex(&digest),
            "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3"
        );
    }

    #[test]
    fn test_abc() {
        let digest = sha512(b"abc");
        assert_eq!(
            to_hex(&digest),
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        );
    }
}
