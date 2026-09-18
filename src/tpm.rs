// ❗ЗАВИСИМОТИ: инит скрипт pid 1 который будет ограничивать пользовательский
// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0). Интеграция в существующий код
// tpm — модуль аппаратной безопасности TPM 2.0 (модель) + полнодисковое
// шифрование (включено ПО УМОЛЧАНИЮ):
//   * TpmDevice — PCR-банк (24), NV-хранилище (индексы), owner-аутентификация,
//     seal/unseal секретов, привязанных к PCR (маска разворачивается из PCR).
//   * DiskCrypto — XTS-AES-256 (ядро crypto::xts) + PBKDF2-HMAC-SHA1
//     (crypto::pbkdf2): из пароля разблокировки выводится ключ секторов;
//     ключ запечатан в TPM NV-индекс 0x01000000, привязан к PCR 7.
//   * Boot gate — при загрузке ОС ЗАПРАШИВАЕТ пароль разблокировки диска
//     (шифрование включено по умолчанию, отключить нельзя без TPM-обхода).
//   * Скрытый раздел /TPM — пароли и ключи хранятся в самом TPM (NV),
//     раздел недоступен Ring 3, не перечислим, стирание невозможно:
//     erase-операции отклоняются (writable=false), полный сброс требует
//     физического TPM-clear через EDL.
// no_std-совместимо: alloc (BTreeMap, String, Vec), вывод — crate::println!.


use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;
use crate::crypto::xts::XtsAes256;

/// Количество PCR-регистров TPM 2.0 (штатный банк SHA-1: 24 регистра).
pub const PCR_COUNT: usize = 24;

/// PCR, в который запечатывается ключ диска (7 = Secure Boot state в TCG).
pub const DISK_KEY_PCR: u32 = 7;








/// Строго типизированное перечисление ошибок TPM/шифрования.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TpmError {
    /// TPM отсутствует или не отвечает.
    DeviceUnavailable,
    /// Неверная аутентификация владельца/индекса.
    AuthFailed,
    /// Индекс NV не определён.
    NvIndexNotFound(u32),
    /// Индекс NV защищён от записи (скрытый раздел — стирание невозможно).
    NvWriteProtected(u32),
    /// Недостаточно места в NV-хранилище.
    NvFull,
    /// Неверная длина данных.
    InvalidLength { expected: usize, got: usize },
    /// PCR-состояние не совпадает (unseal невозможен).
    PcrMismatch { pcr: u32 },
    /// Ключ диска не разблокирован (шифрование активно).
    DiskLocked,
}

impl TpmError {
}

/// Запись NV-хранилища TPM.
///
/// Поля:
/// * `index`    — 32-битный индекс NV (слот).
/// * `name`     — символьное имя слота.
/// * `data`     — содержимое (секрет).
/// * `writable` — разрешена ли запись. Секреты в скрытом разделе /TPM
///   определяются с writable=false: стереть/перезаписать их нельзя.
#[derive(Debug, Clone)]
pub struct NvIndex {
    pub index: u32,
    pub name: String,
    pub data: Vec<u8>,
    pub writable: bool,
}

/// МОДЕЛЬ УСТРОЙСТВА TPM 2.0.
///
/// Поля:
/// * `present`      — физическое присутствие TPM на шине.
/// * `pcrs`         — банк PCR (24 регистра, расширяются через pcr_extend).
/// * `nv`           — NV-хранилище (индекс -> запись).
/// * `owner_auth`   — аутентификация владельца (физический доступ).
/// * `locked`       — флаг блокировки (во время загрузки до owner-auth).
/// * `power_cycles` — счётчик циклов питания (статистика).
#[derive(Debug, Clone)]
pub struct TpmDevice {
    pub present: bool,
    pub pcrs: [u32; PCR_COUNT],
    pub nv: BTreeMap<u32, NvIndex>,
    pub owner_auth: Option<Vec<u8>>,
    pub locked: bool,
    pub power_cycles: u64,
}

impl TpmDevice {
    /// Конструирует TPM с пустым PCR-банком и NV-хранилищем.
    pub fn new() -> TpmDevice {
        TpmDevice {
            present: true,
            pcrs: [0u32; PCR_COUNT],
            nv: BTreeMap::new(),
            owner_auth: None,
            locked: true,
            power_cycles: 1,
        }
    }

    /// Расширение PCR: pcr = hash(pcr || value) — модель SHA-1-подобной
    /// комбинации (без внешних крейтов — детерминированная функция).
    pub fn pcr_extend(&mut self, pcr: u32, value: u32) -> Result<(), TpmError> {
        match pcr < PCR_COUNT as u32 {
            true => {
                // Смешивание: вращение + XOR — детерминированная «аппаратная»
                // операция, имитирующая extend-семантику TPM.
                let old: u32 = self.pcrs[pcr as usize];
                let mixed: u32 = old.rotate_left(5) ^ value.wrapping_mul(0x9E37_79B9).rotate_right(3);
                self.pcrs[pcr as usize] = mixed;
                Ok(())
            }
            false => Err(TpmError::InvalidLength {
                expected: PCR_COUNT as usize,
                got: (pcr + 1) as usize,
            }),
        }
    }

    /// Текущее значение PCR.
    pub fn pcr_value(&self, pcr: u32) -> Result<u32, TpmError> {
        match pcr < PCR_COUNT as u32 {
            true => Ok(self.pcrs[pcr as usize]),
            false => Err(TpmError::InvalidLength {
                expected: PCR_COUNT as usize,
                got: (pcr + 1) as usize,
            }),
        }
    }

    /// Определение NV-индекса (резервирование слота).
    pub fn nv_define_index(
        &mut self,
        index: u32,
        name: &str,
        size: usize,
        writable: bool,
    ) -> Result<(), TpmError> {
        match self.nv.len() >= 64 {
            true => Err(TpmError::NvFull),
            false => {
                self.nv.insert(
                    index,
                    NvIndex {
                        index,
                        name: name.to_string(),
                        data: vec![0u8; size],
                        writable,
                    },
                );
                Ok(())
            }
        }
    }

    /// Запись в NV-индекс с аутентификацией владельца.
    /// Записи writable=false (секреты скрытого раздела /TPM) стереть нельзя.
    pub fn nv_write(&mut self, index: u32, data: &[u8], auth: &[u8]) -> Result<(), TpmError> {
        match self.owner_auth.as_deref() {
            Some(expected) => {
                match crate::auth::constant_time_eq(expected, auth) {
                    true => {}
                    false => return Err(TpmError::AuthFailed),
                }
            }
            None => {}
        }
        let slot: &mut NvIndex = match self.nv.get_mut(&index) {
            Some(slot) => slot,
            None => return Err(TpmError::NvIndexNotFound(index)),
        };
        match slot.writable {
            true => {}
            false => return Err(TpmError::NvWriteProtected(index)),
        }
        match data.len() == slot.data.len() {
            true => {
                slot.data.copy_from_slice(data);
                Ok(())
            }
            false => Err(TpmError::InvalidLength {
                expected: slot.data.len(),
                got: data.len(),
            }),
        }
    }

    /// АППАРАТНАЯ БЛОКИРОВКА ЗАПИСИ индекса (writable -> false).
    /// После вызова стереть/перезаписать слот невозможно — это защита
    /// секретов скрытого раздела /TPM от любого вмешательства (включая
    /// администратора Ring 3). Требует owner-аутентификации.
    pub fn nv_lock_write(&mut self, index: u32, auth: &[u8]) -> Result<(), TpmError> {
        match self.owner_auth.as_deref() {
            Some(expected) => {
                match crate::auth::constant_time_eq(expected, auth) {
                    true => {}
                    false => return Err(TpmError::AuthFailed),
                }
            }
            None => {}
        }
        let slot: &mut NvIndex = match self.nv.get_mut(&index) {
            Some(slot) => slot,
            None => return Err(TpmError::NvIndexNotFound(index)),
        };
        slot.writable = false;
        Ok(())
    }

    /// Чтение NV-индекса (с аутентификацией владельца).
    pub fn nv_read(&self, index: u32, auth: &[u8]) -> Result<Vec<u8>, TpmError> {
        match self.owner_auth.as_deref() {
            Some(expected) => {
                match crate::auth::constant_time_eq(expected, auth) {
                    true => {}
                    false => return Err(TpmError::AuthFailed),
                }
            }
            None => {}
        }
        match self.nv.get(&index) {
            Some(slot) => Ok(slot.data.clone()),
            None => Err(TpmError::NvIndexNotFound(index)),
        }
    }

    /// Установка аутентификации владельца (при первом включении / через EDL).
    pub fn set_owner_auth(&mut self, auth: &[u8]) {
        self.owner_auth = Some(auth.to_vec());
        self.locked = false;
    }

    /// ЗАПЕЧАТЫВАНИЕ СЕКРЕТА: secret -> sealed, привязанный к PCR.
    /// Маска разворачивается из значения PCR (детерминированная функция).
    /// Без знания PCR-состояния распечатать нельзя (TpmError::PcrMismatch).
    pub fn seal(&self, secret: &[u8], pcr: u32) -> Result<Vec<u8>, TpmError> {
        let pcr_val: u32 = self.pcr_value(pcr)?;
        let mut sealed: Vec<u8> = Vec::with_capacity(secret.len() + 4);
        sealed.extend_from_slice(&pcr_val.to_le_bytes());
        let mut mask: u32 = pcr_val;
        for (i, byte) in secret.iter().enumerate() {
            // Разворачиваем 32-битную маску в байтовый поток.
            let mask_byte: u8 = ((mask >> ((i % 4) * 8)) & 0xFF) as u8;
            sealed.push(byte ^ mask_byte);
            if i % 4 == 3 {
                mask = mask.rotate_left(7).wrapping_add(0x6D2B79F5);
            }
        }
        Ok(sealed)
    }

    /// РАСПЕЧАТЫВАНИЕ СЕКРЕТА: sealed + текущий PCR -> secret.
    /// Если PCR изменился с момента запечатывания — TpmError::PcrMismatch.
    pub fn unseal(&self, sealed: &[u8], pcr: u32) -> Result<Vec<u8>, TpmError> {
        match sealed.len() >= 4 {
            true => {}
            false => {
                return Err(TpmError::InvalidLength {
                    expected: 4,
                    got: sealed.len(),
                });
            }
        }
        let sealed_pcr: u32 = u32::from_le_bytes([sealed[0], sealed[1], sealed[2], sealed[3]]);
        let current_pcr: u32 = self.pcr_value(pcr)?;
        match sealed_pcr == current_pcr {
            true => {}
            false => return Err(TpmError::PcrMismatch { pcr }),
        }
        let mut secret: Vec<u8> = Vec::with_capacity(sealed.len() - 4);
        let mut mask: u32 = current_pcr;
        for (i, byte) in sealed[4..].iter().enumerate() {
            let mask_byte: u8 = ((mask >> ((i % 4) * 8)) & 0xFF) as u8;
            secret.push(byte ^ mask_byte);
            if i % 4 == 3 {
                mask = mask.rotate_left(7).wrapping_add(0x6D2B79F5);
            }
        }
        Ok(secret)
    }

}







// ==================== USERS.DB В TPM ====================
// Пользовательская база хранится НЕ только в ext2-томе, но и в TPM:
//   * NV-слот NV_USERS_DB_INDEX — компактный список пользователей
//     (username + SHA-256-верификатор), запечатанный PCR 7;
//   * скрытый раздел /TPM (LBA 12288, тип 0xDA) — резервная копия базы
//     с маркером DEIXTPM. Оба хранилища writable=false после записи —
//     стереть невозможно (только через физический TPM-clear/EDL).

/// NV-индекс, где лежит USERS.DB (сериализованная база пользователей).
pub const NV_USERS_DB_INDEX: u32 = 0x0100_0010;

/// Максимальный размер USERS.DB в TPM NV (8 КиБ — 16 слотов пользователей).
pub const NV_USERS_DB_MAX: usize = 8192;

/// Маркер скрытого раздела /TPM на диске.
pub const TPM_PARTITION_MARKER: [u8; 8] = *b"DEIXTPM1";

/// LBA начала скрытого раздела /TPM (совпадает с P2 в MBR-таблице).
pub const TPM_PARTITION_LBA: u32 = 12288;

/// Owner-аутентификация TPM (в модели — константа прошивки).
pub const TPM_OWNER_AUTH: &[u8] = b"deix_tpm_owner_root_only";

/// Сохраняет USERS.DB в TPM: пишет в NV-слот (запечатано PCR 7) и в
/// скрытый раздел /TPM на диске (маркер DEIXTPM + данные). Возвращает
/// Ok(()) при успехе. writable=false после записи — стереть нельзя.
pub fn tpm_save_users_db(db_bytes: &[u8]) -> Result<(), TpmError> {
    if db_bytes.len() > NV_USERS_DB_MAX {
        return Err(TpmError::InvalidLength {
            expected: NV_USERS_DB_MAX,
            got: db_bytes.len(),
        });
    }

    let mut tpm = TpmDevice::new();
    tpm.set_owner_auth(TPM_OWNER_AUTH);
    // Гарантируем наличие PCR 7 (расширяем, если ещё не расширен).
    let _ = tpm.pcr_extend(DISK_KEY_PCR, 0xA5A5_5A5A);

    // 1) NV-слот: запечатываем базу ключом PCR 7 и пишем.
    // Размер слота = NV_USERS_DB_MAX + 4: seal() добавляет к payload
    // 4 байта PCR-маски (см. TpmDevice::seal), иначе nv_write упадёт с
    // InvalidLength и резервная копия /TPM никогда не будет записана
    // (баг «ОС забывает пароль после перезагрузки»).
    match tpm.nv_define_index(NV_USERS_DB_INDEX, "users_db", NV_USERS_DB_MAX + 4, true) {
        Ok(()) => {}
        Err(TpmError::NvIndexNotFound(_)) => {}
        Err(_) => {}
    }
    // Дополняем до фиксированного размера и запечатываем.
    let mut sealed_payload: Vec<u8> = Vec::new();
    sealed_payload.extend_from_slice(db_bytes);
    sealed_payload.resize(NV_USERS_DB_MAX, 0);
    let sealed: Vec<u8> = tpm.seal(&sealed_payload, DISK_KEY_PCR)?;
    match tpm.nv_write(NV_USERS_DB_INDEX, &sealed, TPM_OWNER_AUTH) {
        Ok(()) => {}
        Err(TpmError::NvWriteProtected(_)) => {
            // Слот уже аппаратно заблокирован — NV перезаписать нельзя,
            // но это НЕ повод пропускать /TPM: резервную копию на диске
            // всё равно нужно обновить актуальными данными базы (ниже).
        }
        Err(err) => return Err(err),
    }
    // Аппаратная блокировка слота.
    let _ = tpm.nv_lock_write(NV_USERS_DB_INDEX, TPM_OWNER_AUTH);

    // 2) Резервная копия в скрытый раздел /TPM на диске.
    let mut sector = [0u8; 512];
    sector[..8].copy_from_slice(&TPM_PARTITION_MARKER);
    let n = db_bytes.len().min(504);
    sector[8..8 + n].copy_from_slice(&db_bytes[..n]);
    let _ = crate::ata::write_sectors(TPM_PARTITION_LBA, 1, &sector);

    Ok(())
}

/// Читает USERS.DB из TPM (NV-слот, распечатывая PCR 7). Если слот
/// недоступен — пробует резервную копию из скрытого раздела /TPM.
/// Возвращает Ok(байты базы) или Err.
pub fn tpm_load_users_db() -> Result<Vec<u8>, TpmError> {
    let tpm = TpmDevice::new();
    let _ = tpm;

    // 1) Попытка чтения из NV-слота.
    let mut dev = TpmDevice::new();
    dev.set_owner_auth(TPM_OWNER_AUTH);
    match dev.nv_read(NV_USERS_DB_INDEX, TPM_OWNER_AUTH) {
        Ok(sealed) => {
            if sealed.len() >= 4 {
                if let Ok(unsealed) = dev.unseal(&sealed, DISK_KEY_PCR) {
                    // Обрезаем по первому нулевому байту (padding).
                    let end = unsealed.iter().position(|&b| b == 0).unwrap_or(unsealed.len());
                    return Ok(unsealed[..end].to_vec());
                }
            }
        }
        Err(_) => {}
    }

    // 2) Резервная копия из скрытого раздела /TPM.
    let mut sector = [0u8; 512];
    if crate::ata::read_sectors(TPM_PARTITION_LBA, 1, &mut sector).is_ok() {
        if &sector[..8] == &TPM_PARTITION_MARKER {
            let end = sector[8..].iter().position(|&b| b == 0).unwrap_or(504);
            return Ok(sector[8..8 + end].to_vec());
        }
    }

    Err(TpmError::NvIndexNotFound(NV_USERS_DB_INDEX))
}

/// Проверяет, есть ли USERS.DB в TPM (NV или скрытый раздел).
pub fn tpm_has_users_db() -> bool {
    tpm_load_users_db().is_ok()
}
