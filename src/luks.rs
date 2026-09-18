//! ЗАГОЛОВОК ТОМА СО СЛОТАМИ ПАРОЛЕЙ — схема в духе LUKS1.
//!
//! ## Что было не так
//!
//! Раньше ключ тома выводился прямо из пароля: `KEY = SHA-512(пароль)`.
//! Отсюда два следствия, оба плохие:
//!
//! * **один пароль на весь том** — второй пользователь не мог
//!   разблокировать диск, его пароль давал другой ключ;
//! * **смена пароля = перешифровка всего тома**, потому что менялся сам
//!   ключ данных.
//!
//! ## Как устроено теперь
//!
//! Как в LUKS: данные шифруются случайным **мастер-ключом**, который
//! генерируется один раз и никогда не выводится из пароля. Сам
//! мастер-ключ хранится в нескольких **слотах**, в каждом — зашифрованный
//! ключом, выведенным из пароля конкретного пользователя.
//!
//! ```text
//!   пароль -> PBKDF2(соль слота, N итераций) -> KEK
//!   KEK -> AES-256 -> расшифровывает мастер-ключ из слота
//!   мастер-ключ -> XTS-AES-256 -> шифрует сектора тома
//! ```
//!
//! Отсюда сразу три свойства, которых не было:
//!
//! * сколько пользователей — столько паролей (до 8 слотов);
//! * смена пароля переписывает **только слот**, том не трогается;
//! * пароль можно отозвать, стерев слот.
//!
//! ## Раскладка заголовка (сектор LBA 4094, 512 байт)
//!
//! ```text
//!   0    8   магия "DEIXLUKS"
//!   8    4   версия формата (1)
//!   12   4   число итераций PBKDF2
//!   16   32  проверочный хеш мастер-ключа (SHA-512, первые 32 байта)
//!   48   16  резерв
//!   64   ... 8 слотов по 56 байт:
//!              0   1   занят (1) / свободен (0)
//!              1   7   резерв
//!              8   16  соль слота (случайная)
//!              24  32  мастер-ключ, зашифрованный KEK
//! ```
//!
//! Заголовок **не шифруется** — в нём нет ни пароля, ни открытого
//! ключа. Ровно так же устроен заголовок LUKS.

use crate::ata;
use crate::crypto::aes::Aes256;
use crate::crypto::pbkdf2::pbkdf2_sha512;
use crate::crypto::sha512;

/// Сектор заголовка. Соседний с MARKER_LBA (4095), тоже до начала
/// ext2-тома (он с LBA 4096).
pub const HEADER_LBA: u32 = 4094;
const MAGIC: [u8; 8] = *b"DEIXLUKS";
const VERSION: u32 = 1;

/// Сколько паролей может открывать том.
pub const MAX_SLOTS: usize = 8;

const SLOT_BASE: usize = 64;
const SLOT_SIZE: usize = 56;
const SALT_LEN: usize = 16;
const MK_LEN: usize = 32;

/// Итерации PBKDF2 по умолчанию.
///
/// 100 000 — значение из LUKS1 (`cryptsetup` подбирает похожее на
/// современном железе). Вход занимает доли секунды, перебор
/// замедляется в 100 000 раз относительно одного хеша.
///
/// Меняется командой `crypt iter <N>`: больше итераций — медленнее
/// перебор, но ровно во столько же раз дольше ваш собственный вход.
/// Обмануть эту связь нельзя, обе стороны считают одну функцию.
pub const DEFAULT_ITERATIONS: u32 = 100_000;

/// Разобранный заголовок тома.
pub struct Header {
    pub iterations: u32,
    /// SHA-512(мастер-ключ)[..32] — по нему проверяем, что пароль верный.
    pub mk_check: [u8; 32],
    pub raw: [u8; 512],
}

impl Header {
    /// Занят ли слот.
    pub fn slot_used(&self, i: usize) -> bool {
        if i >= MAX_SLOTS {
            return false;
        }
        self.raw[SLOT_BASE + i * SLOT_SIZE] == 1
    }

    fn slot_salt(&self, i: usize) -> [u8; SALT_LEN] {
        let o = SLOT_BASE + i * SLOT_SIZE + 8;
        let mut s = [0u8; SALT_LEN];
        s.copy_from_slice(&self.raw[o..o + SALT_LEN]);
        s
    }

    fn slot_encrypted_mk(&self, i: usize) -> [u8; MK_LEN] {
        let o = SLOT_BASE + i * SLOT_SIZE + 24;
        let mut k = [0u8; MK_LEN];
        k.copy_from_slice(&self.raw[o..o + MK_LEN]);
        k
    }

    /// Сколько слотов занято.
    pub fn used_slots(&self) -> usize {
        (0..MAX_SLOTS).filter(|i| self.slot_used(*i)).count()
    }
}

/// Ключ шифрования ключа: PBKDF2 из пароля и соли слота.
fn derive_kek(password: &str, salt: &[u8; SALT_LEN], iterations: u32) -> [u8; 32] {
    let full = pbkdf2_sha512(password.as_bytes(), salt, iterations);
    let mut kek = [0u8; 32];
    kek.copy_from_slice(&full[..32]);
    kek
}

/// Шифрует/расшифровывает мастер-ключ ключом KEK.
///
/// AES-256 в режиме ECB по двум блокам: данных ровно 32 байта, они
/// случайны и не повторяются, поэтому недостатки ECB здесь не
/// проявляются — именно так LUKS1 защищает свой key material.
fn crypt_mk(kek: &[u8; 32], mk: &mut [u8; MK_LEN], encrypt: bool) {
    let aes = Aes256::new(kek);
    for chunk in mk.chunks_mut(16) {
        let mut block = [0u8; 16];
        block.copy_from_slice(chunk);
        if encrypt {
            aes.encrypt_block(&mut block);
        } else {
            aes.decrypt_block(&mut block);
        }
        chunk.copy_from_slice(&block);
    }
}

/// Читает заголовок с диска.
pub fn read_header() -> Option<Header> {
    let mut buf = [0u8; 512];
    if ata::read_sectors(HEADER_LBA, 1, &mut buf).is_err() {
        return None;
    }
    if buf[..8] != MAGIC {
        return None;
    }
    if u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]) != VERSION {
        return None;
    }
    let iterations = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);
    let mut mk_check = [0u8; 32];
    mk_check.copy_from_slice(&buf[16..48]);
    Some(Header {
        iterations,
        mk_check,
        raw: buf,
    })
}

fn write_header(h: &Header) -> Result<(), ()> {
    ata::write_sectors(HEADER_LBA, 1, &h.raw)
}

/// Есть ли на диске заголовок со слотами.
pub fn exists() -> bool {
    read_header().is_some()
}

/// СОЗДАЁТ том: генерирует случайный мастер-ключ и кладёт его в слот 0,
/// запечатав паролем. Возвращает мастер-ключ для немедленного
/// использования.
pub fn create(password: &str, iterations: u32) -> Result<[u8; MK_LEN], ()> {
    let mut mk = [0u8; MK_LEN];
    crate::rng::fill_random(&mut mk);

    let mut buf = [0u8; 512];
    buf[..8].copy_from_slice(&MAGIC);
    buf[8..12].copy_from_slice(&VERSION.to_le_bytes());
    buf[12..16].copy_from_slice(&iterations.to_le_bytes());

    // Проверочный хеш: по нему потом узнаём, верный ли пароль, не
    // расшифровывая весь том.
    let check = sha512::sha512(&mk);
    buf[16..48].copy_from_slice(&check[..32]);

    let mut h = Header {
        iterations,
        mk_check: {
            let mut c = [0u8; 32];
            c.copy_from_slice(&check[..32]);
            c
        },
        raw: buf,
    };

    add_slot_internal(&mut h, 0, password, &mk)?;
    write_header(&h)?;
    Ok(mk)
}

/// Записывает мастер-ключ в слот `idx`, запечатав его паролем.
fn add_slot_internal(
    h: &mut Header,
    idx: usize,
    password: &str,
    mk: &[u8; MK_LEN],
) -> Result<(), ()> {
    if idx >= MAX_SLOTS {
        return Err(());
    }
    let mut salt = [0u8; SALT_LEN];
    crate::rng::fill_random(&mut salt);

    let kek = derive_kek(password, &salt, h.iterations);
    let mut sealed = *mk;
    crypt_mk(&kek, &mut sealed, true);

    let o = SLOT_BASE + idx * SLOT_SIZE;
    h.raw[o] = 1;
    h.raw[o + 8..o + 8 + SALT_LEN].copy_from_slice(&salt);
    h.raw[o + 24..o + 24 + MK_LEN].copy_from_slice(&sealed);
    Ok(())
}

/// Пытается открыть том паролем. Возвращает мастер-ключ и номер слота.
pub fn unlock(password: &str) -> Option<([u8; MK_LEN], usize)> {
    let h = read_header()?;
    for i in 0..MAX_SLOTS {
        if !h.slot_used(i) {
            continue;
        }
        let salt = h.slot_salt(i);
        let kek = derive_kek(password, &salt, h.iterations);
        let mut mk = h.slot_encrypted_mk(i);
        crypt_mk(&kek, &mut mk, false);

        // Сверяем контрольный хеш — так узнаём, тот ли пароль, не
        // трогая данные тома.
        let check = sha512::sha512(&mk);
        if check[..32] == h.mk_check {
            return Some((mk, i));
        }
    }
    None
}

/// ДОБАВЛЯЕТ пароль в свободный слот. Требует любой действующий пароль:
/// без него неоткуда взять мастер-ключ.
pub fn add_password(existing: &str, new_password: &str) -> Result<usize, &'static str> {
    let (mk, _) = unlock(existing).ok_or("неверный текущий пароль")?;
    let mut h = read_header().ok_or("заголовок тома не найден")?;

    let free = (0..MAX_SLOTS)
        .find(|i| !h.slot_used(*i))
        .ok_or("все слоты заняты")?;

    add_slot_internal(&mut h, free, new_password, &mk).map_err(|_| "ошибка слота")?;
    write_header(&h).map_err(|_| "ошибка записи заголовка")?;
    Ok(free)
}

/// УДАЛЯЕТ слот. Последний занятый слот удалить нельзя — иначе том
/// станет невосстановимым.
pub fn remove_slot(existing: &str, idx: usize) -> Result<(), &'static str> {
    let (_, cur) = unlock(existing).ok_or("неверный пароль")?;
    let mut h = read_header().ok_or("заголовок тома не найден")?;

    if idx >= MAX_SLOTS || !h.slot_used(idx) {
        return Err("слот пуст");
    }
    if h.used_slots() <= 1 {
        return Err("это последний пароль — удаление сделает том недоступным");
    }
    if idx == cur {
        return Err("нельзя удалить слот, которым вы вошли");
    }

    let o = SLOT_BASE + idx * SLOT_SIZE;
    // Затираем слот целиком: и флаг, и соль, и запечатанный ключ.
    for b in h.raw[o..o + SLOT_SIZE].iter_mut() {
        *b = 0;
    }
    write_header(&h).map_err(|_| "ошибка записи заголовка")?;
    Ok(())
}

/// Меняет число итераций PBKDF2 и перепечатывает ВСЕ слоты.
///
/// Требует пароль: слоты пересобираются заново, иначе они окажутся
/// выведены со старым числом итераций и перестанут открываться.
pub fn set_iterations(password: &str, iterations: u32) -> Result<(), &'static str> {
    if !(1_000..=50_000_000).contains(&iterations) {
        return Err("допустимо от 1 000 до 50 000 000 итераций");
    }
    let (mk, _) = unlock(password).ok_or("неверный пароль")?;
    let mut h = read_header().ok_or("заголовок тома не найден")?;

    // Слоты, выведенные старым паролем, восстановить нельзя — мы знаем
    // только тот пароль, которым вошли. Поэтому остальные слоты
    // сбрасываются, о чём вызывающий предупреждает пользователя.
    for i in 0..MAX_SLOTS {
        let o = SLOT_BASE + i * SLOT_SIZE;
        for b in h.raw[o..o + SLOT_SIZE].iter_mut() {
            *b = 0;
        }
    }
    h.iterations = iterations;
    h.raw[12..16].copy_from_slice(&iterations.to_le_bytes());
    add_slot_internal(&mut h, 0, password, &mk).map_err(|_| "ошибка слота")?;
    write_header(&h).map_err(|_| "ошибка записи заголовка")?;
    Ok(())
}

// ==================== CLI ====================

/// `crypt <status|addpass|delpass|iter>` — управление паролями тома.
pub fn cmd_crypt(arg: &str) {
    let mut it = arg.trim().splitn(4, ' ');
    let sub = it.next().unwrap_or("");
    let a1 = it.next().unwrap_or("").trim();
    let a2 = it.next().unwrap_or("").trim();

    match sub {
        "status" => {
            match read_header() {
                Some(h) => {
                    crate::println!("  Шифрование тома: XTS-AES-256, заголовок со слотами");
                    crate::println!("  Итераций PBKDF2: {}", h.iterations);
                    crate::println!("  Занято слотов:   {} из {}", h.used_slots(), MAX_SLOTS);
                    for i in 0..MAX_SLOTS {
                        crate::println!(
                            "    слот {}: {}",
                            i,
                            if h.slot_used(i) { "занят" } else { "свободен" }
                        );
                    }
                }
                None => {
                    if crate::crypto_storage::is_encryption_enabled() {
                        crate::println!("  Том зашифрован по СТАРОЙ схеме (ключ = хеш пароля).");
                        crate::println!("  Несколько паролей недоступны: нужен заголовок слотов.");
                    } else {
                        crate::println!("  Том не зашифрован.");
                    }
                }
            }
        }
        "addpass" if !a1.is_empty() && !a2.is_empty() => {
            match add_password(a1, a2) {
                Ok(slot) => crate::println!("  Пароль добавлен в слот {}.", slot),
                Err(e) => crate::println!("  ОШИБКА: {}", e),
            }
        }
        "delpass" if !a1.is_empty() && !a2.is_empty() => {
            match a2.parse::<usize>() {
                Ok(idx) => match remove_slot(a1, idx) {
                    Ok(()) => crate::println!("  Слот {} удалён.", idx),
                    Err(e) => crate::println!("  ОШИБКА: {}", e),
                },
                Err(_) => crate::println!("  Номер слота должен быть числом."),
            }
        }
        "iter" if !a1.is_empty() && !a2.is_empty() => match a2.parse::<u32>() {
            Ok(n) => {
                crate::println!("  ВНИМАНИЕ: все прочие пароли будут сброшены,");
                crate::println!("  останется только тот, которым вы сейчас вошли.");
                match set_iterations(a1, n) {
                    Ok(()) => crate::println!("  Итераций теперь {}. Вход станет медленнее.", n),
                    Err(e) => crate::println!("  ОШИБКА: {}", e),
                }
            }
            Err(_) => crate::println!("  Число итераций должно быть числом."),
        },
        _ => {
            crate::println!("crypt status                      - слоты и параметры тома");
            crate::println!("crypt addpass <текущий> <новый>   - добавить пароль (до 8)");
            crate::println!("crypt delpass <пароль> <слот>     - удалить слот");
            crate::println!("crypt iter <пароль> <N>           - сменить итерации PBKDF2");
            crate::println!("");
            crate::println!("Итерации: больше = медленнее перебор, но во столько же раз");
            crate::println!("дольше ваш вход. По умолчанию 100000 (как в LUKS1).");
        }
    }
}

/// Добавляет пароль, используя мастер-ключ УЖЕ РАЗБЛОКИРОВАННОГО тома.
///
/// Нужен для `useradd`: пароль администратора там неизвестен, но том
/// открыт, значит мастер-ключ лежит в памяти движка. Без этого новый
/// пользователь получил бы аккаунт без доступа к зашифрованным данным.
pub fn add_password_with_master(new_password: &str) -> Result<usize, &'static str> {
    let mk = crate::crypto_storage::current_master_key().ok_or("том не разблокирован")?;
    let mut h = read_header().ok_or("заголовок тома не найден")?;

    let free = (0..MAX_SLOTS)
        .find(|i| !h.slot_used(*i))
        .ok_or("все слоты заняты")?;

    add_slot_internal(&mut h, free, new_password, &mk).map_err(|_| "ошибка слота")?;
    write_header(&h).map_err(|_| "ошибка записи заголовка")?;
    Ok(free)
}
