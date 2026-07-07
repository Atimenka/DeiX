//! Криптографические примитивы. Изначально написаны для WPA2-PSK
//! (SHA-1, HMAC-SHA1, PBKDF2, PRF), позже дополнены SHA-256 (хэширование
//! паролей, см. auth.rs) и AES-256+XTS+SHA-512 (шифрование диска, см.
//! crypto_storage.rs). Всё написано с нуля (без внешних crate) и
//! проверено по официальным тестовым векторам NIST/RFC/FIPS — см.
//! `#[cfg(test)]` в каждом файле, а также сверено побайтово с эталонными
//! реализациями (Python `hashlib`/`cryptography`, обёртки над OpenSSL) —
//! подробности методологии в комментариях crypto_storage.rs.
//!
//! AES-CCMP (шифрование Wi-Fi трафика после установления PTK/GTK) здесь
//! не реализован — отдельный большой кусок работы, не связанный с
//! шифрованием диска.

pub mod aes;
pub mod hmac_sha1;
pub mod pbkdf2;
pub mod prf;
pub mod sha1;
pub mod sha256;
pub mod sha512;
pub mod xts;
