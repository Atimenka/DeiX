//! PBKDF2-HMAC-SHA512 — вывод ключа из пароля (RFC 8018, RFC 2104).
//!
//! ## Зачем это нужно
//!
//! Раньше ключ тома выводился как `SHA-512(пароль)` — один проход, без
//! соли. Это две отдельные дыры:
//!
//! * **нет соли** — одинаковый пароль даёт одинаковый ключ на любой
//!   машине, значит работают заранее посчитанные (радужные) таблицы;
//! * **один проход** — перебор идёт со скоростью хеширования, миллионы
//!   паролей в секунду на обычном железе.
//!
//! PBKDF2 лечит обе: соль делает каждый том уникальным, а повторение
//! хеша N раз замедляет перебор ровно в N раз. Это тот же механизм,
//! что использует LUKS в Linux.
//!
//! ## Честное ограничение
//!
//! PBKDF2 плохо сопротивляется GPU: он почти не требует памяти, и
//! видеокарта считает тысячи потоков параллельно. Современные схемы
//! (Argon2, scrypt) специально жрут память, чтобы GPU не давал выигрыша.
//! Мы берём PBKDF2, потому что он стандарт LUKS1 и реализуется поверх
//! уже имеющегося SHA-512 без новых зависимостей.

use crate::crypto::sha512;

/// Размер блока SHA-512 в байтах (1024 бита).
const BLOCK_LEN: usize = 128;
/// Размер выхода SHA-512.
const DIGEST_LEN: usize = 64;

/// HMAC-SHA512 (RFC 2104): `HMAC(K, m) = H((K' ^ opad) || H((K' ^ ipad) || m))`,
/// где `K'` — ключ, дополненный нулями до размера блока (или сначала
/// хешированный, если он длиннее блока). Хранит готовые состояния после
/// `K' ^ ipad` и `K' ^ opad`.
/// PBKDF2 считает HMAC с одним ключом сто тысяч раз: поглощать ipad и
/// opad (по блоку SHA-512) заново в каждой итерации — половина всей
/// работы впустую. Результат побитово тот же, что при полном пересчёте.
struct HmacSha512 {
    inner: sha512::Sha512,
    outer: sha512::Sha512,
}

impl HmacSha512 {
    fn new(key: &[u8]) -> Self {
        let mut k = [0u8; BLOCK_LEN];

        if key.len() > BLOCK_LEN {
            // Слишком длинный ключ сначала сжимаем хешем — так требует RFC.
            let h = sha512::sha512(key);
            k[..DIGEST_LEN].copy_from_slice(&h);
        } else {
            k[..key.len()].copy_from_slice(key);
        }

        let mut ipad = [0x36u8; BLOCK_LEN];
        let mut opad = [0x5Cu8; BLOCK_LEN];
        for i in 0..BLOCK_LEN {
            ipad[i] ^= k[i];
            opad[i] ^= k[i];
        }

        let mut inner = sha512::Sha512::new();
        inner.update(&ipad);
        let mut outer = sha512::Sha512::new();
        outer.update(&opad);

        // Затираем копию ключа: она лежала на стеке в открытом виде.
        for b in k.iter_mut() {
            *b = 0;
        }
        for b in ipad.iter_mut() {
            *b = 0;
        }
        for b in opad.iter_mut() {
            *b = 0;
        }

        HmacSha512 { inner, outer }
    }

    /// `H((K ^ opad) || H((K ^ ipad) || message))`.
    fn mac(&self, message: &[u8]) -> [u8; DIGEST_LEN] {
        let mut inner = self.inner.clone();
        inner.update(message);
        let inner_digest = inner.finalize();

        let mut outer = self.outer.clone();
        outer.update(&inner_digest);
        outer.finalize()
    }

    /// То же для сообщения ровно в один выход SHA-512, заданного словами:
    /// две компрессии без буферов и преобразований байтов.
    fn mac_words(&self, message: &[u64; 8]) -> [u64; 8] {
        let inner_digest = self.inner.digest_of_words(message);
        self.outer.digest_of_words(&inner_digest)
    }
}

impl Drop for HmacSha512 {
    fn drop(&mut self) {
        self.inner.wipe();
        self.outer.wipe();
    }
}

/// PBKDF2-HMAC-SHA512, выход ровно 64 байта (один блок).
///
/// `iterations` задаёт стойкость: перебор замедляется ровно во столько
/// же раз. Значение по умолчанию и его цена обсуждаются в
/// `crypto_storage.rs`.
pub fn pbkdf2_sha512(password: &[u8], salt: &[u8], iterations: u32) -> [u8; DIGEST_LEN] {
    // Блок 1: U1 = HMAC(password, salt || INT(1))
    let mut salt_block = alloc::vec::Vec::with_capacity(salt.len() + 4);
    salt_block.extend_from_slice(salt);
    salt_block.extend_from_slice(&1u32.to_be_bytes());

    let hmac = HmacSha512::new(password);
    let first = hmac.mac(&salt_block);
    let mut u = [0u64; 8];
    for (word, chunk) in u.iter_mut().zip(first.chunks_exact(8)) {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(chunk);
        *word = u64::from_be_bytes(bytes);
    }
    let mut result = u;

    // T = U1 ^ U2 ^ ... ^ Uc, где Ui = HMAC(password, U(i-1))
    for _ in 1..iterations.max(1) {
        u = hmac.mac_words(&u);
        for i in 0..8 {
            result[i] ^= u[i];
        }
    }

    let mut out = [0u8; DIGEST_LEN];
    for (chunk, word) in out.chunks_exact_mut(8).zip(result.iter()) {
        chunk.copy_from_slice(&word.to_be_bytes());
    }
    for word in u.iter_mut() {
        *word = 0;
    }
    out
}
