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

/// Ключевой расписание (key schedule) для AES-256: NR+1 = 15 раундовых
/// ключей по 16 байт каждый (240 байт всего).
pub struct Aes256 {
    round_keys: [[u8; 16]; NR + 1],
}

fn xtime(x: u8) -> u8 {
    if x & 0x80 != 0 {
        (x << 1) ^ 0x1b
    } else {
        x << 1
    }
}

fn gmul(a: u8, b: u8) -> u8 {
    let mut a = a;
    let mut b = b;
    let mut p = 0u8;
    for _ in 0..8 {
        if b & 1 != 0 {
            p ^= a;
        }
        a = xtime(a);
        b >>= 1;
    }
    p
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

        Aes256 { round_keys }
    }

    /// Шифрует ровно один 16-байтный блок на месте (ECB-уровня примитив
    /// — НЕ используйте напрямую для шифрования данных длиннее блока,
    /// без режима сцепления это уязвимо; см. xts.rs для настоящего
    /// шифрования секторов).
    pub fn encrypt_block(&self, block: &mut [u8; 16]) {
        add_round_key(block, &self.round_keys[0]);

        for round in 1..NR {
            sub_bytes(block);
            shift_rows(block);
            mix_columns(block);
            add_round_key(block, &self.round_keys[round]);
        }

        sub_bytes(block);
        shift_rows(block);
        add_round_key(block, &self.round_keys[NR]);
    }

    /// Затирает ключевое расписание (round keys, производные от
    /// исходного AES-ключа) через write_volatile — гарантированно не
    /// удаляется компилятором как "мёртвая запись" (в отличие от
    /// обычного присваивания), см. использование в
    /// crypto_storage.rs::clear_master_key.
    pub fn zeroize(&mut self) {
        for round_key in self.round_keys.iter_mut() {
            for byte in round_key.iter_mut() {
                unsafe { core::ptr::write_volatile(byte, 0) };
            }
        }
    }

    pub fn decrypt_block(&self, block: &mut [u8; 16]) {
        add_round_key(block, &self.round_keys[NR]);
        inv_shift_rows(block);
        inv_sub_bytes(block);

        for round in (1..NR).rev() {
            add_round_key(block, &self.round_keys[round]);
            inv_mix_columns(block);
            inv_shift_rows(block);
            inv_sub_bytes(block);
        }

        add_round_key(block, &self.round_keys[0]);
    }
}

fn sub_word(word: [u8; 4]) -> [u8; 4] {
    [SBOX[word[0] as usize], SBOX[word[1] as usize], SBOX[word[2] as usize], SBOX[word[3] as usize]]
}

fn rot_word(word: [u8; 4]) -> [u8; 4] {
    [word[1], word[2], word[3], word[0]]
}

fn add_round_key(block: &mut [u8; 16], round_key: &[u8; 16]) {
    for i in 0..16 {
        block[i] ^= round_key[i];
    }
}

fn sub_bytes(block: &mut [u8; 16]) {
    for b in block.iter_mut() {
        *b = SBOX[*b as usize];
    }
}

fn inv_sub_bytes(block: &mut [u8; 16]) {
    for b in block.iter_mut() {
        *b = INV_SBOX[*b as usize];
    }
}

/// Состояние AES хранится как массив байт в column-major порядке:
/// state[r + 4*c] соответствует строке r, столбцу c. ShiftRows сдвигает
/// строку r влево на r позиций.
fn shift_rows(block: &mut [u8; 16]) {
    let s = *block;
    for r in 1..4 {
        for c in 0..4 {
            block[r + 4 * c] = s[r + 4 * ((c + r) % 4)];
        }
    }
}

fn inv_shift_rows(block: &mut [u8; 16]) {
    let s = *block;
    for r in 1..4 {
        for c in 0..4 {
            block[r + 4 * c] = s[r + 4 * ((c + 4 - r) % 4)];
        }
    }
}

fn mix_columns(block: &mut [u8; 16]) {
    for c in 0..4 {
        let col = [block[4 * c], block[4 * c + 1], block[4 * c + 2], block[4 * c + 3]];
        block[4 * c] = gmul(col[0], 2) ^ gmul(col[1], 3) ^ col[2] ^ col[3];
        block[4 * c + 1] = col[0] ^ gmul(col[1], 2) ^ gmul(col[2], 3) ^ col[3];
        block[4 * c + 2] = col[0] ^ col[1] ^ gmul(col[2], 2) ^ gmul(col[3], 3);
        block[4 * c + 3] = gmul(col[0], 3) ^ col[1] ^ col[2] ^ gmul(col[3], 2);
    }
}

fn inv_mix_columns(block: &mut [u8; 16]) {
    for c in 0..4 {
        let col = [block[4 * c], block[4 * c + 1], block[4 * c + 2], block[4 * c + 3]];
        block[4 * c] = gmul(col[0], 0x0e) ^ gmul(col[1], 0x0b) ^ gmul(col[2], 0x0d) ^ gmul(col[3], 0x09);
        block[4 * c + 1] = gmul(col[0], 0x09) ^ gmul(col[1], 0x0e) ^ gmul(col[2], 0x0b) ^ gmul(col[3], 0x0d);
        block[4 * c + 2] = gmul(col[0], 0x0d) ^ gmul(col[1], 0x09) ^ gmul(col[2], 0x0e) ^ gmul(col[3], 0x0b);
        block[4 * c + 3] = gmul(col[0], 0x0b) ^ gmul(col[1], 0x0d) ^ gmul(col[2], 0x09) ^ gmul(col[3], 0x0e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
