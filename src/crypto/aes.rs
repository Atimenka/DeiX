//! AES-256 (FIPS-197) — блочный шифр, реализованный с нуля, без внешних
//! crate (та же методология, что и sha1.rs/sha256.rs в этом же модуле).
//! Это "строительный блок": сам по себе AES шифрует только ровно 16-байтные
//! блоки в самом слабом (ECB) режиме, поэтому для настоящего шифрования
//! диска он используется ВНУТРИ режима XTS (см. xts.rs) — именно так же
//! это устроено в настоящем Linux dm-crypt/cryptsetup.
//!
//! Реализована только 256-битная версия (14 раундов) — это единственный
//! размер ключа, который нам нужен (см. crypto_storage.rs: XTS требует
//! ДВА независимых AES-ключа, и мы используем AES-256 для обоих половин
//! ради максимальной стойкости, соответствует `cryptsetup --key-size 512`
//! для aes-xts-plain64, где итоговый 512-битный ключ делится на два
//! 256-битных AES-ключа).
//!
//! Проверено побайтово против официальных тестовых векторов NIST
//! (FIPS-197, Appendix C.3 — AES-256) и, дополнительно, сверено с Python
//! `cryptography` (обёртка над OpenSSL) на нескольких сообщениях — см.
//! auth.rs/crypto_storage.rs комментарии о методологии тестирования.

const NB: usize = 4; // количество 32-битных слов в состоянии (всегда 4 для AES)
const NK: usize = 8; // количество 32-битных слов в ключе (8 для AES-256)
const NR: usize = 14; // количество раундов (14 для AES-256)

const SBOX: [u8; 256] = [
    0x63, 0x7c, 0x77, 0x7b, 0xf2, 0x6b, 0x6f, 0xc5, 0x30, 0x01, 0x67, 0x2b, 0xfe, 0xd7, 0xab, 0x76,
    0xca, 0x82, 0xc9, 0x7d, 0xfa, 0x59, 0x47, 0xf0, 0xad, 0xd4, 0xa2, 0xaf, 0x9c, 0xa4, 0x72, 0xc0,
    0xb7, 0xfd, 0x93, 0x26, 0x36, 0x3f, 0xf7, 0xcc, 0x34, 0xa5, 0xe5, 0xf1, 0x71, 0xd8, 0x31, 0x15,
    0x04, 0xc7, 0x23, 0xc3, 0x18, 0x96, 0x05, 0x9a, 0x07, 0x12, 0x80, 0xe2, 0xeb, 0x27, 0xb2, 0x75,
    0x09, 0x83, 0x2c, 0x1a, 0x1b, 0x6e, 0x5a, 0xa0, 0x52, 0x3b, 0xd6, 0xb3, 0x29, 0xe3, 0x2f, 0x84,
    0x53, 0xd1, 0x00, 0xed, 0x20, 0xfc, 0xb1, 0x5b, 0x6a, 0xcb, 0xbe, 0x39, 0x4a, 0x4c, 0x58, 0xcf,
    0xd0, 0xef, 0xaa, 0xfb, 0x43, 0x4d, 0x33, 0x85, 0x45, 0xf9, 0x02, 0x7f, 0x50, 0x3c, 0x9f, 0xa8,
    0x51, 0xa3, 0x40, 0x8f, 0x92, 0x9d, 0x38, 0xf5, 0xbc, 0xb6, 0xda, 0x21, 0x10, 0xff, 0xf3, 0xd2,
    0xcd, 0x0c, 0x13, 0xec, 0x5f, 0x97, 0x44, 0x17, 0xc4, 0xa7, 0x7e, 0x3d, 0x64, 0x5d, 0x19, 0x73,
    0x60, 0x81, 0x4f, 0xdc, 0x22, 0x2a, 0x90, 0x88, 0x46, 0xee, 0xb8, 0x14, 0xde, 0x5e, 0x0b, 0xdb,
    0xe0, 0x32, 0x3a, 0x0a, 0x49, 0x06, 0x24, 0x5c, 0xc2, 0xd3, 0xac, 0x62, 0x91, 0x95, 0xe4, 0x79,
    0xe7, 0xc8, 0x37, 0x6d, 0x8d, 0xd5, 0x4e, 0xa9, 0x6c, 0x56, 0xf4, 0xea, 0x65, 0x7a, 0xae, 0x08,
    0xba, 0x78, 0x25, 0x2e, 0x1c, 0xa6, 0xb4, 0xc6, 0xe8, 0xdd, 0x74, 0x1f, 0x4b, 0xbd, 0x8b, 0x8a,
    0x70, 0x3e, 0xb5, 0x66, 0x48, 0x03, 0xf6, 0x0e, 0x61, 0x35, 0x57, 0xb9, 0x86, 0xc1, 0x1d, 0x9e,
    0xe1, 0xf8, 0x98, 0x11, 0x69, 0xd9, 0x8e, 0x94, 0x9b, 0x1e, 0x87, 0xe9, 0xce, 0x55, 0x28, 0xdf,
    0x8c, 0xa1, 0x89, 0x0d, 0xbf, 0xe6, 0x42, 0x68, 0x41, 0x99, 0x2d, 0x0f, 0xb0, 0x54, 0xbb, 0x16,
];

const INV_SBOX: [u8; 256] = build_inv_sbox();

const fn build_inv_sbox() -> [u8; 256] {
    let mut inv = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        inv[SBOX[i] as usize] = i as u8;
        i += 1;
    }
    inv
}

const RCON: [u8; 15] = [
    0x00, 0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36, 0x6c, 0xd8, 0xab, 0x4d,
];

/// Ключевое расписание AES-256: NR+1 = 15 раундовых ключей по четыре
/// 32-битных слова. `enc` — для шифрования, `dec` — для «эквивалентного
/// обратного шифра» (FIPS-197 §5.3.5): раундовые ключи в обратном порядке,
/// средние пропущены через InvMixColumns.
pub struct Aes256 {
    enc: [[u32; 4]; NR + 1],
    dec: [[u32; 4]; NR + 1],
}

const fn xtime(x: u8) -> u8 {
    if x & 0x80 != 0 {
        (x << 1) ^ 0x1b
    } else {
        x << 1
    }
}

const fn gmul(a: u8, b: u8) -> u8 {
    let mut a = a;
    let mut b = b;
    let mut p = 0u8;
    let mut i = 0;
    while i < 8 {
        if b & 1 != 0 {
            p ^= a;
        }
        a = xtime(a);
        b >>= 1;
        i += 1;
    }
    p
}

/// T-таблицы: SubBytes + MixColumns одного байта целиком, столбец — слово
/// big-endian. Остальные три таблицы получаются поворотом на 8/16/24 бита.
/// Раунд — 16 обращений к таблице и XOR вместо побайтовых SubBytes,
/// ShiftRows и MixColumns: при программном AES в эмуляторе это основной
/// выигрыш для XTS-шифрования диска.
const fn build_te0() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let s = SBOX[i];
        t[i] = ((gmul(s, 2) as u32) << 24) | ((s as u32) << 16) | ((s as u32) << 8) | gmul(s, 3) as u32;
        i += 1;
    }
    t
}

/// То же для обратного шифра: InvSubBytes + InvMixColumns.
const fn build_td0() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let s = INV_SBOX[i];
        t[i] = ((gmul(s, 14) as u32) << 24)
            | ((gmul(s, 9) as u32) << 16)
            | ((gmul(s, 13) as u32) << 8)
            | gmul(s, 11) as u32;
        i += 1;
    }
    t
}

static TE0: [u32; 256] = build_te0();
static TD0: [u32; 256] = build_td0();

#[inline(always)]
fn te(x: u32, rot: u32) -> u32 {
    TE0[(x & 0xff) as usize].rotate_right(rot)
}

#[inline(always)]
fn td(x: u32, rot: u32) -> u32 {
    TD0[(x & 0xff) as usize].rotate_right(rot)
}

#[inline(always)]
fn sb(x: u32) -> u32 {
    SBOX[(x & 0xff) as usize] as u32
}

#[inline(always)]
fn isb(x: u32) -> u32 {
    INV_SBOX[(x & 0xff) as usize] as u32
}

fn load_block(block: &[u8; 16]) -> [u32; 4] {
    [
        u32::from_be_bytes([block[0], block[1], block[2], block[3]]),
        u32::from_be_bytes([block[4], block[5], block[6], block[7]]),
        u32::from_be_bytes([block[8], block[9], block[10], block[11]]),
        u32::from_be_bytes([block[12], block[13], block[14], block[15]]),
    ]
}

fn store_block(s: [u32; 4], block: &mut [u8; 16]) {
    for c in 0..4 {
        block[4 * c..4 * c + 4].copy_from_slice(&s[c].to_be_bytes());
    }
}

/// Байтовые раундовые ключи -> слова big-endian.
fn key_words(round_keys: &[[u8; 16]], out: &mut [[u32; 4]]) {
    for (r, rk) in round_keys.iter().enumerate() {
        out[r] = load_block(rk);
    }
}

/// InvMixColumns для слова раундового ключа (через TD0 и SBOX: TD0[S[b]]
/// даёт вклад байта b в InvMixColumns без отдельной таблицы).
fn inv_mix_word(w: u32) -> u32 {
    TD0[sb(w >> 24) as usize]
        ^ TD0[sb(w >> 16) as usize].rotate_right(8)
        ^ TD0[sb(w >> 8) as usize].rotate_right(16)
        ^ TD0[sb(w) as usize].rotate_right(24)
}

fn encrypt_words(rk: &[[u32; 4]], block: &mut [u8; 16]) {
    let rounds = rk.len() - 1;
    let mut s = load_block(block);
    for c in 0..4 {
        s[c] ^= rk[0][c];
    }
    for r in 1..rounds {
        let k = &rk[r];
        s = [
            TE0[(s[0] >> 24) as usize] ^ te(s[1] >> 16, 8) ^ te(s[2] >> 8, 16) ^ te(s[3], 24) ^ k[0],
            TE0[(s[1] >> 24) as usize] ^ te(s[2] >> 16, 8) ^ te(s[3] >> 8, 16) ^ te(s[0], 24) ^ k[1],
            TE0[(s[2] >> 24) as usize] ^ te(s[3] >> 16, 8) ^ te(s[0] >> 8, 16) ^ te(s[1], 24) ^ k[2],
            TE0[(s[3] >> 24) as usize] ^ te(s[0] >> 16, 8) ^ te(s[1] >> 8, 16) ^ te(s[2], 24) ^ k[3],
        ];
    }
    let k = &rk[rounds];
    let out = [
        (sb(s[0] >> 24) << 24) | (sb(s[1] >> 16) << 16) | (sb(s[2] >> 8) << 8) | sb(s[3]),
        (sb(s[1] >> 24) << 24) | (sb(s[2] >> 16) << 16) | (sb(s[3] >> 8) << 8) | sb(s[0]),
        (sb(s[2] >> 24) << 24) | (sb(s[3] >> 16) << 16) | (sb(s[0] >> 8) << 8) | sb(s[1]),
        (sb(s[3] >> 24) << 24) | (sb(s[0] >> 16) << 16) | (sb(s[1] >> 8) << 8) | sb(s[2]),
    ];
    store_block([out[0] ^ k[0], out[1] ^ k[1], out[2] ^ k[2], out[3] ^ k[3]], block);
}

fn decrypt_words(dk: &[[u32; 4]], block: &mut [u8; 16]) {
    let rounds = dk.len() - 1;
    let mut s = load_block(block);
    for c in 0..4 {
        s[c] ^= dk[0][c];
    }
    for r in 1..rounds {
        let k = &dk[r];
        s = [
            TD0[(s[0] >> 24) as usize] ^ td(s[3] >> 16, 8) ^ td(s[2] >> 8, 16) ^ td(s[1], 24) ^ k[0],
            TD0[(s[1] >> 24) as usize] ^ td(s[0] >> 16, 8) ^ td(s[3] >> 8, 16) ^ td(s[2], 24) ^ k[1],
            TD0[(s[2] >> 24) as usize] ^ td(s[1] >> 16, 8) ^ td(s[0] >> 8, 16) ^ td(s[3], 24) ^ k[2],
            TD0[(s[3] >> 24) as usize] ^ td(s[2] >> 16, 8) ^ td(s[1] >> 8, 16) ^ td(s[0], 24) ^ k[3],
        ];
    }
    let k = &dk[rounds];
    let out = [
        (isb(s[0] >> 24) << 24) | (isb(s[3] >> 16) << 16) | (isb(s[2] >> 8) << 8) | isb(s[1]),
        (isb(s[1] >> 24) << 24) | (isb(s[0] >> 16) << 16) | (isb(s[3] >> 8) << 8) | isb(s[2]),
        (isb(s[2] >> 24) << 24) | (isb(s[1] >> 16) << 16) | (isb(s[0] >> 8) << 8) | isb(s[3]),
        (isb(s[3] >> 24) << 24) | (isb(s[2] >> 16) << 16) | (isb(s[1] >> 8) << 8) | isb(s[0]),
    ];
    store_block([out[0] ^ k[0], out[1] ^ k[1], out[2] ^ k[2], out[3] ^ k[3]], block);
}

impl Aes256 {
    /// Строит ключевое расписание из 32-байтного (256-битного) ключа.
    pub fn new(key: &[u8; 32]) -> Self {
        let mut w = [[0u8; 4]; NB * (NR + 1)];

        for i in 0..NK {
            w[i] = [key[4 * i], key[4 * i + 1], key[4 * i + 2], key[4 * i + 3]];
        }

        for i in NK..NB * (NR + 1) {
            let mut temp = w[i - 1];
            if i % NK == 0 {
                temp = sub_word(rot_word(temp));
                temp[0] ^= RCON[i / NK];
            } else if NK > 6 && i % NK == 4 {
                temp = sub_word(temp);
            }
            for j in 0..4 {
                w[i][j] = w[i - NK][j] ^ temp[j];
            }
        }

        let mut round_keys = [[0u8; 16]; NR + 1];
        for r in 0..=NR {
            for c in 0..4 {
                let word = w[r * 4 + c];
                round_keys[r][c * 4..c * 4 + 4].copy_from_slice(&word);
            }
        }

        let mut enc = [[0u32; 4]; NR + 1];
        key_words(&round_keys, &mut enc);
        let mut dec = [[0u32; 4]; NR + 1];
        dec[0] = enc[NR];
        dec[NR] = enc[0];
        for r in 1..NR {
            let src = enc[NR - r];
            dec[r] = [inv_mix_word(src[0]), inv_mix_word(src[1]), inv_mix_word(src[2]), inv_mix_word(src[3])];
        }
        for rk in round_keys.iter_mut() {
            for b in rk.iter_mut() {
                unsafe { core::ptr::write_volatile(b, 0) };
            }
        }
        Aes256 { enc, dec }
    }

    /// Шифрует ровно один 16-байтный блок на месте (ECB-уровня примитив
    /// — НЕ используйте напрямую для шифрования данных длиннее блока,
    /// без режима сцепления это уязвимо; см. xts.rs для настоящего
    /// шифрования секторов).
    pub fn encrypt_block(&self, block: &mut [u8; 16]) {
        encrypt_words(&self.enc, block);
    }

    /// Затирает ключевое расписание (round keys, производные от
    /// исходного AES-ключа) через write_volatile — гарантированно не
    /// удаляется компилятором как "мёртвая запись" (в отличие от
    /// обычного присваивания), см. использование в
    /// crypto_storage.rs::clear_master_key.
    pub fn zeroize(&mut self) {
        for round_key in self.enc.iter_mut().chain(self.dec.iter_mut()) {
            for word in round_key.iter_mut() {
                unsafe { core::ptr::write_volatile(word, 0) };
            }
        }
    }

    pub fn decrypt_block(&self, block: &mut [u8; 16]) {
        decrypt_words(&self.dec, block);
    }
}

/// AES-128 (FIPS-197: NK=4, NR=10), только шифрование блока.
/// Нужен для AES-128-GCM — обязательного сьюта TLS 1.3
/// (TLS_AES_128_GCM_SHA256, RFC 8446 §9.1). Переиспользует SBOX и
/// преобразования раунда AES-256 выше.
pub struct Aes128 {
    enc: [[u32; 4]; 11],
}

impl Aes128 {
    pub fn new(key: &[u8; 16]) -> Self {
        const NK128: usize = 4;
        const NR128: usize = 10;
        let mut w = [[0u8; 4]; NB * (NR128 + 1)];

        for i in 0..NK128 {
            w[i] = [key[4 * i], key[4 * i + 1], key[4 * i + 2], key[4 * i + 3]];
        }

        for i in NK128..NB * (NR128 + 1) {
            let mut temp = w[i - 1];
            if i % NK128 == 0 {
                temp = sub_word(rot_word(temp));
                temp[0] ^= RCON[i / NK128];
            }
            for j in 0..4 {
                w[i][j] = w[i - NK128][j] ^ temp[j];
            }
        }

        let mut round_keys = [[0u8; 16]; NR128 + 1];
        for r in 0..=NR128 {
            for c in 0..4 {
                round_keys[r][c * 4..c * 4 + 4].copy_from_slice(&w[r * 4 + c]);
            }
        }

        let mut enc = [[0u32; 4]; NR128 + 1];
        key_words(&round_keys, &mut enc);
        Aes128 { enc }
    }

    pub fn encrypt_block(&self, block: &mut [u8; 16]) {
        encrypt_words(&self.enc, block);
    }
}

fn sub_word(word: [u8; 4]) -> [u8; 4] {
    [SBOX[word[0] as usize], SBOX[word[1] as usize], SBOX[word[2] as usize], SBOX[word[3] as usize]]
}

fn rot_word(word: [u8; 4]) -> [u8; 4] {
    [word[1], word[2], word[3], word[0]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aes128_fips197_vector() {
        // Официальный тестовый вектор NIST FIPS-197 Appendix C.1 (AES-128).
        let key: [u8; 16] = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
            0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
        ];
        let mut block: [u8; 16] = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77,
            0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
        ];
        let aes = Aes128::new(&key);
        aes.encrypt_block(&mut block);
        assert_eq!(
            block,
            [
                0x69, 0xc4, 0xe0, 0xd8, 0x6a, 0x7b, 0x04, 0x30,
                0xd8, 0xcd, 0xb7, 0x80, 0x70, 0xb4, 0xc5, 0x5a,
            ]
        );
    }

    #[test]
    fn test_aes256_fips197_vector() {
        // Официальный тестовый вектор NIST FIPS-197 Appendix C.3 (AES-256).
        let key: [u8; 32] = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
            0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f,
        ];
        let mut block: [u8; 16] = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
        ];
        let expected: [u8; 16] = [
            0x8e, 0xa2, 0xb7, 0xca, 0x51, 0x67, 0x45, 0xbf, 0xea, 0xfc, 0x49, 0x90, 0x4b, 0x49, 0x60, 0x89,
        ];

        let aes = Aes256::new(&key);
        aes.encrypt_block(&mut block);
        assert_eq!(block, expected);

        aes.decrypt_block(&mut block);
        assert_eq!(
            block,
            [0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]
        );
    }
}
