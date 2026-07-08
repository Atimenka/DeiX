#![allow(dead_code)]
//! XTS-AES (IEEE P1619 / NIST SP 800-38E) — режим шифрования блочных
//! устройств поверх AES (см. aes.rs), реализован с нуля. Это ТОТ ЖЕ
//! режим и та же схема, что использует `cryptsetup --cipher
//! aes-xts-plain64` — то есть настоящий Linux dm-crypt в режиме "plain"
//! (не LUKS — без бинарного заголовка с метаданными, ключ выводится
//! прямо из пароля при каждом открытии, см. crypto_storage.rs).
//!
//! IV/"tweak" вычисляется по схеме "plain64": номер сектора как 64-битное
//! число в little-endian, дополненное нулями до 16 байт — точно так же,
//! как это делает ядро Linux (dm-crypt) для aes-xts-plain64.
//!
//! XTS требует ДВА независимых AES-ключа: ключ 1 шифрует сами данные,
//! ключ 2 шифрует только "tweak" (номер сектора) для получения
//! per-block модификатора. У нас используется AES-256 для обеих
//! половин (соответствует `cryptsetup --key-size 512` — 512-битный общий
//! ключ делится на два 256-битных AES-ключа).
//!
//! Проверено побайтово против эталонной реализации Python `cryptography`
//! (обёртка над OpenSSL EVP, та же имплементация XTS, которую использует
//! настоящий Linux/OpenSSL) — см. комментарии в crypto_storage.rs с
//! тестовыми векторами.

use super::aes::Aes256;

pub const SECTOR_SIZE: usize = 512;

pub struct XtsAes256 {
    cipher1: Aes256, // шифрует данные
    cipher2: Aes256, // шифрует tweak (номер сектора)
}

impl XtsAes256 {
    /// `key` — 64 байта (512 бит): первые 32 байта это ключ данных,
    /// последние 32 байта — ключ tweak-а. Именно так делится ключ в
    /// XTS согласно спецификации (не "первая половина суммы", а именно
    /// раздел пополам двух независимых AES-256 ключей).
    pub fn new(key: &[u8; 64]) -> Self {
        let mut key1 = [0u8; 32];
        let mut key2 = [0u8; 32];
        key1.copy_from_slice(&key[..32]);
        key2.copy_from_slice(&key[32..]);

        XtsAes256 {
            cipher1: Aes256::new(&key1),
            cipher2: Aes256::new(&key2),
        }
    }

    /// Вычисляет начальный tweak для заданного номера сектора (схема
    /// plain64: 64-битный LE номер сектора, дополненный нулями до 16
    /// байт), затем шифрует его вторым AES-ключом — это и есть исходный
    /// "T" в терминологии спецификации XTS перед последовательным
    /// умножением на примитивный элемент GF(2^128) для каждого 16-байтного
    /// под-блока внутри сектора.
    fn initial_tweak(&self, sector_num: u64) -> [u8; 16] {
        let mut tweak = [0u8; 16];
        tweak[..8].copy_from_slice(&sector_num.to_le_bytes());
        self.cipher2.encrypt_block(&mut tweak);
        tweak
    }

    /// Умножает tweak на примитивный элемент GF(2^128) (x, представленный
    /// как сдвиг влево с приведением по модулю многочлена
    /// x^128 + x^7 + x^2 + x + 1) — это стандартная операция "alpha
    /// multiplication" из спецификации XTS, применяемая между
    /// последовательными 16-байтными блоками внутри одного сектора.
    fn multiply_tweak(tweak: &mut [u8; 16]) {
        let mut carry = 0u8;
        for byte in tweak.iter_mut() {
            let new_carry = (*byte >> 7) & 1;
            *byte = (*byte << 1) | carry;
            carry = new_carry;
        }
        if carry != 0 {
            tweak[0] ^= 0x87; // x^128 = x^7+x^2+x+1 (0b10000111) в GF(2^128)
        }
    }

    fn xor_block(a: &mut [u8; 16], b: &[u8; 16]) {
        for i in 0..16 {
            a[i] ^= b[i];
        }
    }

    /// Шифрует ОДИН сектор (обычно 512 байт, ровно SECTOR_SIZE) на
    /// месте. Длина данных должна быть кратна 16 байтам (у нас всегда
    /// целые 512-байтные сектора, поэтому это условие выполняется
    /// автоматически и не требует ciphertext-stealing для неполных
    /// последних блоков, как того требует полная спецификация XTS для
    /// произвольных длин).
    pub fn encrypt_sector(&self, sector_num: u64, data: &mut [u8]) {
        let mut tweak = self.initial_tweak(sector_num);
        let mut offset = 0;
        while offset + 16 <= data.len() {
            let mut block = [0u8; 16];
            block.copy_from_slice(&data[offset..offset + 16]);

            Self::xor_block(&mut block, &tweak);
            self.cipher1.encrypt_block(&mut block);
            Self::xor_block(&mut block, &tweak);

            data[offset..offset + 16].copy_from_slice(&block);
            Self::multiply_tweak(&mut tweak);
            offset += 16;
        }
    }

    pub fn decrypt_sector(&self, sector_num: u64, data: &mut [u8]) {
        let mut tweak = self.initial_tweak(sector_num);
        let mut offset = 0;
        while offset + 16 <= data.len() {
            let mut block = [0u8; 16];
            block.copy_from_slice(&data[offset..offset + 16]);

            Self::xor_block(&mut block, &tweak);
            self.cipher1.decrypt_block(&mut block);
            Self::xor_block(&mut block, &tweak);

            data[offset..offset + 16].copy_from_slice(&block);
            Self::multiply_tweak(&mut tweak);
            offset += 16;
        }
    }

    /// Затирает оба внутренних AES-ключевых расписания через
    /// write_volatile — используется crypto_storage.rs при выключении
    /// системы, чтобы мастер-ключ шифрования диска не оставался
    /// восстановимым из дампа оперативной памяти после halt. Обычное
    /// присваивание/Drop НЕ даёт такой гарантии (см. подробное
    /// объяснение в crypto_storage.rs::clear_master_key).
    pub fn zeroize(&mut self) {
        self.cipher1.zeroize();
        self.cipher2.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xts_roundtrip() {
        let key = [0x42u8; 64];
        let xts = XtsAes256::new(&key);

        let mut data = [0xABu8; SECTOR_SIZE];
        let original = data;

        xts.encrypt_sector(7, &mut data);
        assert_ne!(data, original);

        xts.decrypt_sector(7, &mut data);
        assert_eq!(data, original);
    }
}
