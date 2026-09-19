//! Модуль авторизации пользователей DeiX — регистрация аккаунта и вход
//! по паролю. Хранение паролей в открытом виде НИГДЕ не допускается: на
//! диск и в память кладётся только SHA-256(соль || пароль), сама соль
//! (16 случайных байт на аккаунт, см. rng.rs) хранится рядом с хэшем —
//! это стандартная и общепринятая схема (тот же принцип, что и в
//! /etc/shadow в Linux, только там используется sha256crypt/bcrypt/yescrypt
//! с несколькими раундами; здесь — один раунд SHA-256, что ЧЕСТНО menее
//! устойчиво к перебору на GPU, чем настоящие KDF с намеренным
//! замедлением, но полностью устраняет главную уязвимость — саму
//! возможность прочитать чей-то пароль из файла или дампа памяти).
//!
//! ФОРМАТ ФАЙЛА `USERS.DB` (простой построчный текстовый формат — не
//! бинарный, чтобы содержимое можно было прочитать командой `cat` и
//! понять, что реально записано, в целях прозрачности и отладки):
//!
//!   username:hex(salt,16 bytes):hex(sha256(salt || password),32 bytes)\n
//!
//! Например:
//!   admin:a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08
//!
//! Пароль НИКОГДА не хранится и никогда не может быть восстановлен из
//! хэша — SHA-256 является однонаправленной функцией: единственный
//! способ "проверить" пароль — заново вычислить хэш от введённого
//! значения и сравнить байт-в-байт с сохранённым (см. verify_login).

use crate::crypto::sha256;
use crate::ext2;
use crate::rng;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

const USERS_DB_FILE: &str = "USERS.DB";
const SALT_LEN: usize = 16;
const HASH_LEN: usize = sha256::DIGEST_LEN; // 32 байта

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    DiskError,
    UserAlreadyExists,
    UserNotFound,
    WrongPassword,
    InvalidUsername,
    CorruptDatabase,
}

struct UserRecord {
    username: String,
    salt: [u8; SALT_LEN],
    hash: [u8; HASH_LEN],
}

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(hex_digit(b >> 4));
        s.push(hex_digit(b & 0x0f));
    }
    s
}

fn hex_digit(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        _ => (b'a' + (n - 10)) as char,
    }
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = hex_value(bytes[i])?;
        let lo = hex_value(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Some(out)
}

fn hex_value(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Загружает и парсит USERS.DB. Если файла ещё нет (первый запуск,
/// ни одного аккаунта не создано) — возвращает пустой список, это не
/// ошибка.
fn load_users() -> Result<Vec<UserRecord>, AuthError> {
    // USERS.DB хранится в TPM (NV-слот + скрытый раздел /TPM) и дублируется
    // в ext2-том. Приоритет: TPM (защищено PCR), затем ext2.
    let data: alloc::vec::Vec<u8> = match crate::tpm::tpm_load_users_db() {
        Ok(d) if !d.is_empty() => d,
        _ => match ext2::read_file(USERS_DB_FILE) {
            Ok(d) => d,
            Err(ext2::Ext2Error::FileNotFound) | Err(ext2::Ext2Error::NotFormatted) => {
                return Ok(Vec::new());
            }
            Err(_) => return Err(AuthError::DiskError),
        },
    };

    let text = core::str::from_utf8(&data).map_err(|_| AuthError::CorruptDatabase)?;
    let mut users = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(3, ':');
        let username = parts.next().ok_or(AuthError::CorruptDatabase)?;
        let salt_hex = parts.next().ok_or(AuthError::CorruptDatabase)?;
        let hash_hex = parts.next().ok_or(AuthError::CorruptDatabase)?;

        let salt_bytes = from_hex(salt_hex).ok_or(AuthError::CorruptDatabase)?;
        let hash_bytes = from_hex(hash_hex).ok_or(AuthError::CorruptDatabase)?;
        if salt_bytes.len() != SALT_LEN || hash_bytes.len() != HASH_LEN {
            return Err(AuthError::CorruptDatabase);
        }

        let mut salt = [0u8; SALT_LEN];
        salt.copy_from_slice(&salt_bytes);
        let mut hash = [0u8; HASH_LEN];
        hash.copy_from_slice(&hash_bytes);

        users.push(UserRecord {
            username: username.to_string(),
            salt,
            hash,
        });
    }

    Ok(users)
}

fn save_users(users: &[UserRecord]) -> Result<(), AuthError> {
    if !ext2::is_formatted() {
        ext2::format().map_err(|_| AuthError::DiskError)?;
    }

    let mut text = String::new();
    for u in users {
        text.push_str(&u.username);
        text.push(':');
        text.push_str(&to_hex(&u.salt));
        text.push(':');
        text.push_str(&to_hex(&u.hash));
        text.push('\n');
    }

    // 1) ext2-том (рабочая копия).
    ext2::write_file(USERS_DB_FILE, text.as_bytes()).map_err(|_| AuthError::DiskError)?;
    // 2) TPM: NV-слот + скрытый раздел /TPM (защищённая копия).
    let _ = crate::tpm::tpm_save_users_db(text.as_bytes());
    Ok(())
}

fn validate_username(username: &str) -> Result<(), AuthError> {
    if username.is_empty() || username.len() > 32 {
        return Err(AuthError::InvalidUsername);
    }
    if !username.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err(AuthError::InvalidUsername);
    }
    Ok(())
}

/// Вычисляет SHA-256(salt || password) — именно в этом порядке (соль
/// ПЕРЕД паролем), как это делают большинство salted-hash схем, чтобы
/// два пользователя с одинаковым паролем получили совершенно разные
/// хэши (защита от rainbow-таблиц и от простого сравнения хэшей между
/// аккаунтами, которое иначе выдало бы "у этих двух юзеров одинаковый
/// пароль", даже не зная сам пароль).
fn hash_password(salt: &[u8; SALT_LEN], password: &str) -> [u8; HASH_LEN] {
    let mut hasher = sha256::Sha256::new();
    hasher.update(salt);
    hasher.update(password.as_bytes());
    hasher.finalize()
}

/// Постоянное по времени сравнение двух хэшей — обычное `==` для срезов
/// байт в Rust завершается досрочно при первом несовпадающем байте, что
/// теоретически позволяет атаке по времени выполнения (timing attack)
/// подбирать хэш побайтово, измеряя, на каком байте сравнение прервалось
/// быстрее/медленнее. Сравниваем ВСЕ байты всегда, накапливая результат
/// через побитовое ИЛИ разниц — количество итераций и операций не
/// зависит от того, где именно первое расхождение.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Создаёт нового пользователя с паролем `password`. Генерирует новую
/// случайную соль (см. rng.rs), хэширует пароль вместе с ней и
/// записывает итоговую пару (соль, хэш) в USERS.DB — сам пароль нигде
/// не сохраняется и не остаётся в этой функции дольше, чем нужно для
/// вычисления хэша.
pub fn create_user(username: &str, password: &str) -> Result<(), AuthError> {
    validate_username(username)?;

    let mut users = load_users()?;
    if users.iter().any(|u| u.username == username) {
        return Err(AuthError::UserAlreadyExists);
    }

    let mut salt = [0u8; SALT_LEN];
    rng::fill_random(&mut salt);
    let hash = hash_password(&salt, password);

    users.push(UserRecord {
        username: username.to_string(),
        salt,
        hash,
    });

    save_users(&users)
}

/// Проверяет логин/пароль. Возвращает Ok(()) при успехе — пароль в
/// открытом виде используется только внутри этой функции для повторного
/// хэширования и сравнения, после чего сразу выходит из области
/// видимости (компилятор Rust освобождает String при выходе из scope;
/// честно отметим, что мы НЕ делаем явную перезапись памяти пароля
/// через write_volatile, как это сделано для мастер-ключа шифрования в
/// crypto_storage.rs — короткоживущий пароль пользователя это отдельный,
/// менее критичный случай, но сама структура кода уже минимизирует время
/// его жизни в памяти до одного вызова хэширования).
pub fn verify_login(username: &str, password: &str) -> Result<(), AuthError> {
    let users = load_users()?;
    let user = users
        .iter()
        .find(|u| u.username == username)
        .ok_or(AuthError::UserNotFound)?;

    let computed_hash = hash_password(&user.salt, password);

    if constant_time_eq(&computed_hash, &user.hash) {
        Ok(())
    } else {
        Err(AuthError::WrongPassword)
    }
}

/// Меняет пароль уже существующего пользователя — генерирует НОВУЮ соль
/// (не переиспользует старую), пересчитывает хэш и перезаписывает запись.
pub fn change_password(username: &str, old_password: &str, new_password: &str) -> Result<(), AuthError> {
    verify_login(username, old_password)?;

    let mut users = load_users()?;
    let user = users
        .iter_mut()
        .find(|u| u.username == username)
        .ok_or(AuthError::UserNotFound)?;

    let mut new_salt = [0u8; SALT_LEN];
    rng::fill_random(&mut new_salt);
    user.salt = new_salt;
    user.hash = hash_password(&new_salt, new_password);

    save_users(&users)
}

/// true, если хотя бы один аккаунт уже создан — используется CLI, чтобы
/// решить, нужно ли при загрузке предлагать "создать первый аккаунт"
/// или сразу "войти в систему".
pub fn has_any_users() -> bool {
    load_users().map(|u| !u.is_empty()).unwrap_or(false)
}

pub fn list_usernames() -> Result<Vec<String>, AuthError> {
    Ok(load_users()?.into_iter().map(|u| u.username).collect())
}

// ==================== Экран входа при загрузке ====================
//
// Вызывается один раз из kernel_main() ДО того, как открывается обычный
// CLI-приглашение (cli::run()). Если ни одного аккаунта ещё не создано
// (первый запуск свежеустановленной системы) — предлагает создать
// первый аккаунт; иначе запрашивает логин и пароль и не пускает дальше,
// пока они не совпадут с хранящимся хэшем. Пароль вводится С МАСКИРОВКОЙ
// (звёздочки вместо символов на экране) — сами символы, разумеется,
// всё равно проходят через обычную клавиатурную очередь в виде обычных
// ASCII-байт (иначе их было бы не собрать в строку для хэширования),
// маскировка — это только то, что видно на экране, не куда-то ещё
// записываемая копия.

use crate::keyboard;
use crate::{print, println};

const MAX_INPUT: usize = 64;

/// Считывает одну строку с клавиатуры/COM1 с маскировкой звёздочками (для ввода пароля).
pub fn read_line_masked() -> String {
    read_line(true)
}

pub fn read_line(mask: bool) -> String {
    let mut buf = [0u8; MAX_INPUT];
    let mut len = 0usize;

    loop {
        // Ввод: неблокирующий опрос КЛАВИАТУРЫ (PS/2) и ПОСЛЕДОВАТЕЛЬНОГО
        // ПОРТА COM1 (headless-режим QEMU: -serial stdio). Если оба канала
        // пусты — ждём следующей итерации (не зависаем на блокирующем
        // чтении клавиатуры).
        let c: Option<u8> = if crate::serial::is_data_ready() {
            Some(crate::serial::read_byte())
        } else {
            keyboard::try_read_char()
        };
        let c: u8 = match c {
            Some(c) => c,
            None => {
                // Небольшая пауза (без прерываний) и повторный опрос.
                unsafe { core::arch::asm!("nop"); }
                continue;
            }
        };
        match c {
            b'\n' => {
                print!("\n");
                break;
            }
            0x08 => {
                if len > 0 {
                    len -= 1;
                    print!("\u{8}");
                }
            }
            byte if byte >= 0x20 && byte < 0x7F => {
                if len < MAX_INPUT {
                    buf[len] = byte;
                    len += 1;
                    if mask {
                        print!("*");
                    } else {
                        let s = [byte];
                        if let Ok(s) = core::str::from_utf8(&s) {
                            print!("{}", s);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    core::str::from_utf8(&buf[..len]).unwrap_or("").to_string()
}


/// Основной цикл экрана входа. Возвращает имя успешно вошедшего
/// пользователя.
pub fn run_login_screen() -> String {
    // ЕДИНЫЙ ПАРОЛЬ: пароль учётной записи пользователя является также
    // ключом шифрования диска. Отдельного экрана разблокировки диска нет:
    // при вводе пароля на экране входа ОС сначала расшифровывает том
    // (crypto_storage::try_unlock), затем проверяет пользователя.
    println!();
    println!("=====================================");
    println!("        DeiX v0.2-beta - Login        ");
    println!("=====================================");
    if crate::crypto_storage::is_encryption_enabled() {
        println!("Disk is encrypted (XTS-AES-256). Your account password unlocks it.");
    }

    if !has_any_users() {
        println!();
        println!("No user accounts exist yet. Let's create the first one.");
        loop {
            print!("Choose a username: ");
            let username = read_line(false);
            if username.trim().is_empty() {
                println!("Username cannot be empty.");
                continue;
            }

            print!("Choose a password: ");
            let password = read_line(true);
            if password.is_empty() {
                println!("Password cannot be empty.");
                continue;
            }

            match create_user(username.trim(), &password) {
                Ok(()) => {
                    // Первая настройка: диск зашифровывается паролем аккаунта
                    // (XTS-AES-256). Все последующие загрузки требуют этот
                    // же пароль для входа и разблокировки диска.
                    match crate::crypto_storage::enable_encryption(&password) {
                        Ok(()) => {
                            println!("Disk encryption ENABLED (XTS-AES-256). Key = account password.");
                        }
                        Err(_) => {
                            println!("WARNING: could not enable disk encryption (disk error).");
                        }
                    }
                    println!("Account '{}' created. Logging in...", username.trim());
                    return username.trim().to_string();
                }
                Err(AuthError::InvalidUsername) => {
                    println!("Invalid username (use letters, digits, '_' or '-', max 32 chars).");
                }
                Err(_) => {
                    println!("Failed to create account (disk error). Try again.");
                }
            }
        }
    }

    loop {
        print!("Username: ");
        let username = read_line(false);

        print!("Password: ");
        let password = read_line(true);

        // Если диск зашифрован — пароль должен сначала разблокировать том.
        if crate::crypto_storage::is_encryption_enabled() {
            if !crate::crypto_storage::try_unlock(&password) {
                println!("Invalid username or password. Try again.");
                let _ = crate::sound::play_ui(crate::sound::UiSound::Error);
                continue;
            }
        }

        match verify_login(username.trim(), &password) {
            Ok(()) => {
                println!("Login successful. Welcome, {}!", username.trim());
                return username.trim().to_string();
            }
            Err(AuthError::WrongPassword) | Err(AuthError::UserNotFound) => {
                println!("Invalid username or password. Try again.");
                let _ = crate::sound::play_ui(crate::sound::UiSound::Error);
            }
            Err(_) => {
                println!("Login failed (disk error). Try again.");
                let _ = crate::sound::play_ui(crate::sound::UiSound::Error);
            }
        }
    }
}

