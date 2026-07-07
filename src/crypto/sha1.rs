//! SHA-1 (FIPS 180-4). Нужен как основа для HMAC-SHA1 и PBKDF2, которые
//! используются в WPA2/WPA-PSK для деривации ключей (PMK/PTK/GTK).
//!
//! Реализация написана с нуля (без внешних crate — у нас no_std/no_alloc-
//! дружественный минимальный набор зависимостей) и проверена по
//! официальным тестовым векторам NIST (см. tests в конце файла).

pub const DIGEST_LEN: usize = 20;
const BLOCK_LEN: usize = 64;

pub struct Sha1 {
    state: [u32; 5],
    buffer: [u8; BLOCK_LEN],
    buffer_len: usize,
    total_len: u64,
}

impl Sha1 {
    pub fn new() -> Self {
        Sha1 {
            state: [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0],
            buffer: [0; BLOCK_LEN],
            buffer_len: 0,
            total_len: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.total_len += data.len() as u64;

        if self.buffer_len > 0 {
            let need = BLOCK_LEN - self.buffer_len;
            let take = need.min(data.len());
            self.buffer[self.buffer_len..self.buffer_len + take].copy_from_slice(&data[..take]);
            self.buffer_len += take;
            data = &data[take..];

            if self.buffer_len == BLOCK_LEN {
                let block = self.buffer;
                self.process_block(&block);
                self.buffer_len = 0;
            }
        }

        while data.len() >= BLOCK_LEN {
            let mut block = [0u8; BLOCK_LEN];
            block.copy_from_slice(&data[..BLOCK_LEN]);
            self.process_block(&block);
            data = &data[BLOCK_LEN..];
        }

        if !data.is_empty() {
            self.buffer[..data.len()].copy_from_slice(data);
            self.buffer_len = data.len();
        }
    }

    pub fn finalize(mut self) -> [u8; DIGEST_LEN] {
        let bit_len = self.total_len * 8;

        // Padding: 0x80, затем нули, затем 8 байт длины в битах (big-endian),
        // так чтобы итоговая длина была кратна 64 байтам.
        self.update_no_len_track(&[0x80]);
        while self.buffer_len != 56 {
            self.update_no_len_track(&[0x00]);
        }
        self.update_no_len_track(&bit_len.to_be_bytes());

        let mut out = [0u8; DIGEST_LEN];
        for (i, word) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    /// Как update(), но не считает добавленные байты в total_len — нужно
    /// только для служебного padding в finalize().
    fn update_no_len_track(&mut self, mut data: &[u8]) {
        if self.buffer_len > 0 {
            let need = BLOCK_LEN - self.buffer_len;
            let take = need.min(data.len());
            self.buffer[self.buffer_len..self.buffer_len + take].copy_from_slice(&data[..take]);
            self.buffer_len += take;
            data = &data[take..];

            if self.buffer_len == BLOCK_LEN {
                let block = self.buffer;
                self.process_block(&block);
                self.buffer_len = 0;
            }
        }

        while data.len() >= BLOCK_LEN {
            let mut block = [0u8; BLOCK_LEN];
            block.copy_from_slice(&data[..BLOCK_LEN]);
            self.process_block(&block);
            data = &data[BLOCK_LEN..];
        }

        if !data.is_empty() {
            self.buffer[..data.len()].copy_from_slice(data);
            self.buffer_len = data.len();
        }
    }

    fn process_block(&mut self, block: &[u8; BLOCK_LEN]) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let (mut a, mut b, mut c, mut d, mut e) = (
            self.state[0],
            self.state[1],
            self.state[2],
            self.state[3],
            self.state[4],
        );

        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDCu32),
                _ => (b ^ c ^ d, 0xCA62C1D6u32),
            };

            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
    }
}

pub fn sha1(data: &[u8]) -> [u8; DIGEST_LEN] {
    let mut hasher = Sha1::new();
    hasher.update(data);
    hasher.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_hex(bytes: &[u8]) -> alloc::string::String {
        use core::fmt::Write;
        let mut s = alloc::string::String::new();
        for b in bytes {
            write!(s, "{:02x}", b).unwrap();
        }
        s
    }

    #[test]
    fn test_sha1_empty() {
        // Официальный тестовый вектор NIST для пустой строки.
        let digest = sha1(b"");
        assert_eq!(to_hex(&digest), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    }

    #[test]
    fn test_sha1_abc() {
        // Официальный тестовый вектор NIST FIPS 180-4, "abc".
        let digest = sha1(b"abc");
        assert_eq!(to_hex(&digest), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn test_sha1_long() {
        // Официальный тестовый вектор NIST: 56-байтная строка.
        let digest = sha1(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq");
        assert_eq!(to_hex(&digest), "84983e441c3bd26ebaae4aa1f95129e5e54670f1");
    }
}
