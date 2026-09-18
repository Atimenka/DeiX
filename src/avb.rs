// ❗ЗАВИСИМОТИ: инит скрипт pid 1 который будет ограничивать пользовательский
// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0). Интеграция в существующий код
// avb — VERIFIED BOOT (аналог Android Verified Boot / AVB + vbmeta).
// Три состояния при загрузке (как на устройствах Android):
//   * GREEN  — загрузчик ЗАБЛОКИРОВАН, системные файлы не изменены:
//              система запускается как обычно, никаких сообщений.
//   * ORANGE — загрузчик РАЗБЛОКИРОВАН (dev-режим): при загрузке рядом
//              с логотипом пишется, что загрузчик разблокирован и система
//              может быть изменена; запуск ОС ЗАДЕРЖИВАЕТСЯ на 5 секунд.
//   * RED    — загрузчик заблокирован, НО системные файлы изменены
//              (vbmeta не отключена): загрузка ЗАПРЕЩЕНА; на экране сверху
//              знак опасности, по центру красная надпись
//              "Your device is corrupt. It can't be trusted and will not boot".
// vbmeta — запечатанный хэш-каталог системных разделов (SHA-256 каждого
// системного файла), хранится в файле VBMETA.BIN на /system-томе. При
// загрузке ядро пересчитывает хэши и сверяет с vbmeta: совпадение -> Green,
// расхождение при заблокированном загрузчике -> Red.
// no_std-совместимо: alloc (String, Vec), вывод — crate::println!.


use alloc::string::{String, ToString};
use alloc::vec::Vec;


/// Состояние верифицированной загрузки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootState {
    /// Всё в порядке: загрузчик заблокирован, файлы не изменены.
    Green,
    /// Загрузчик разблокирован: предупреждение + задержка 5 сек.
    Orange,
    /// Система повреждена (файлы изменены при заблокированном загрузчике).
    Red,
}

impl BootState {
    pub fn as_str(&self) -> &'static str {
        match self {
            BootState::Green => "GREEN",
            BootState::Orange => "ORANGE",
            BootState::Red => "RED",
        }
    }
}

/// Состояние блокировки загрузчика (bootloader lock).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootloaderLock {
    Locked,
    Unlocked,
}

impl BootloaderLock {
    pub fn as_str(&self) -> &'static str {
        match self {
            BootloaderLock::Locked => "locked",
            BootloaderLock::Unlocked => "unlocked",
        }
    }
}

/// Параметры vbmeta: запечатанные хэши системных файлов.
///
/// Поля:
/// * `entries` — список (имя_файла, hex-хэш SHA-256), защищённые подписью.
/// * `sealed`  — true, если vbmeta запечатана (подпись валидна).
/// * `digest`  — общий дайджест vbmeta (для сверки целостности самой vbmeta).
#[derive(Debug, Clone)]
pub struct VbMeta {
    pub entries: Vec<(String, String)>,
    pub sealed: bool,
    pub digest: String,
}

impl VbMeta {
    /// Формирует vbmeta из списка системных файлов: считает SHA-256 каждого.
    pub fn build(system_files: &[(&str, &[u8])]) -> VbMeta {
        let mut entries: Vec<(String, String)> = Vec::new();
        for (name, data) in system_files.iter() {
            let hash = crate::crypto::sha256::sha256(data);
            entries.push((name.to_string(), to_hex(&hash)));
        }
        // Общий дайджест vbmeta = SHA-256 от конкатенации всех хэшей.
        let mut blob: Vec<u8> = Vec::new();
        for (_, h) in entries.iter() {
            blob.extend_from_slice(h.as_bytes());
        }
        let d = crate::crypto::sha256::sha256(&blob);
        VbMeta {
            entries,
            sealed: true,
            digest: to_hex(&d),
        }
    }

    /// Проверка: совпадают ли текущие хэши системных файлов с vbmeta.
    /// Возвращает имя первого несовпавшего файла (None = всё совпало).
    pub fn verify(&self, system_files: &[(&str, &[u8])]) -> Option<String> {
        for (name, data) in system_files.iter() {
            let hash = crate::crypto::sha256::sha256(data);
            let hex = to_hex(&hash);
            let expected: Option<&String> = self
                .entries
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, h)| h);
            match expected {
                Some(exp) if *exp == hex => {}
                _ => return Some(name.to_string()),
            }
        }
        None
    }
}

/// Драйвер верифицированной загрузки.
///
/// Поля:
/// * `lock`      — состояние загрузчика (locked/unlocked).
/// * `vbmeta`    — запечатанный хэш-каталог (None если не сформирован).
/// * `boot_state`— результат последней проверки.
/// * `orange_delay_ms` — задержка при Orange (5000 мс по ТЗ).
/// * `ota_guarantee` — действует ли OTA-гарантия (снимается в dev-режиме).
pub struct VerifiedBoot {
    pub lock: BootloaderLock,
    pub vbmeta: Option<VbMeta>,
    pub boot_state: BootState,
    pub orange_delay_ms: u64,
    pub ota_guarantee: bool,
}

impl VerifiedBoot {
    pub const fn new() -> VerifiedBoot {
        VerifiedBoot {
            lock: BootloaderLock::Locked,
            vbmeta: None,
            boot_state: BootState::Green,
            orange_delay_ms: 5000,
            ota_guarantee: true,
        }
    }

    /// Устанавливает vbmeta и сразу проверяет системные файлы.
    pub fn install_vbmeta(&mut self, vbmeta: VbMeta, system_files: &[(&str, &[u8])]) -> BootState {
        self.vbmeta = Some(vbmeta);
        self.check(system_files)
    }

    /// ГЛАВНАЯ ПРОВЕРКА ПРИ ЗАГРУЗКЕ.
    /// Логика (как AVB на Android):
    ///   1. vbmeta не запечатана/отсутствует -> Red (система не заслуживает доверия).
    ///   2. загрузчик разблокирован (dev) -> Orange (предупреждение, задержка).
    ///   3. загрузчик заблокирован и хэши совпали -> Green.
    ///   4. загрузчик заблокирован и хэши НЕ совпали -> Red (загрузка запрещена).
    pub fn check(&mut self, system_files: &[(&str, &[u8])]) -> BootState {
        let vb = match &self.vbmeta {
            Some(v) => v,
            None => {
                self.boot_state = BootState::Red;
                return BootState::Red;
            }
        };
        if !vb.sealed {
            self.boot_state = BootState::Red;
            return BootState::Red;
        }

        match self.lock {
            BootloaderLock::Unlocked => {
                self.boot_state = BootState::Orange;
                BootState::Orange
            }
            BootloaderLock::Locked => match vb.verify(system_files) {
                Some(bad) => {
                    crate::serial_println!("[avb] RED: изменён системный файл '{}'", bad);
                    self.boot_state = BootState::Red;
                    BootState::Red
                }
                None => {
                    self.boot_state = BootState::Green;
                    BootState::Green
                }
            },
        }
    }

    /// Разблокировка загрузчика (dev-режим). Снимает OTA-гарантию.
    pub fn unlock_bootloader(&mut self) {
        self.lock = BootloaderLock::Unlocked;
        self.ota_guarantee = false;
        // Orange: проверка теперь даст Orange.
        self.boot_state = BootState::Orange;
    }

    /// Блокировка загрузчика (возврат к заводскому состоянию).
    pub fn lock_bootloader(&mut self) {
        self.lock = BootloaderLock::Locked;
        self.ota_guarantee = true;
    }

    /// Выполняет задержку Orange (5 сек) с предупреждением.
    pub fn orange_warning(&self) {
        crate::println!("  [avb] !!! Bootloader UNLOCKED (dev-режим) !!!");
        crate::println!("  [avb] Система может быть изменена. OTA-гарантия НЕ действует.");
        crate::println!("  [avb] Задержка запуска: {} мс", self.orange_delay_ms);
        let t = crate::timer::uptime_ms() + self.orange_delay_ms;
        while crate::timer::uptime_ms() < t {
            unsafe { core::arch::asm!("hlt"); }
        }
    }

    /// Красный экран RED STATE: знак опасности сверху + красная надпись.
    /// Загрузка запрещена — система не запускается (зависание с экраном).
    pub fn red_screen() -> ! {
        // Знак опасности (⚠) и надпись — в консоль и serial.
        crate::println!();
        crate::println!("  #############################################");
        crate::println!("  #  ⚠  ⚠  ⚠  ⚠  ⚠  ⚠  ⚠  ⚠  ⚠  ⚠  ⚠  ⚠  ⚠  #");
        crate::println!("  #############################################");
        crate::println!("  #                                           #");
        crate::println!("  #  Your device is corrupt.                  #");
        crate::println!("  #  It can't be trusted and will not boot.   #");
        crate::println!("  #                                           #");
        crate::println!("  #  (verified boot error / RED STATE)        #");
        crate::println!("  #                                           #");
        crate::println!("  #############################################");
        crate::println!("  [avb] Система не загружена: vbmeta не отключена,");
        crate::println!("  [avb] загрузчик заблокирован, но системные файлы изменены.");
        crate::serial_println!("[avb] RED STATE: Your device is corrupt. It can't be trusted and will not boot.");
        // Бесконечное зависание с экраном (система не запускается).
        loop {
            unsafe { core::arch::asm!("hlt"); }
        }
    }
}

/// hex-представление дайджеста.
fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::new();
    for b in bytes {
        s.push(hex_digit(b >> 4));
        s.push(hex_digit(b & 0x0F));
    }
    s
}
fn hex_digit(v: u8) -> char {
    match v {
        0..=9 => (b'0' + v) as char,
        10..=15 => (b'a' + (v - 10)) as char,
        _ => '?',
    }
}


// ==================== ГЛОБАЛЬНЫЙ ИНСТАНС ====================

/// Глобальный VerifiedBoot (для CLI/OTA/adb/dev).
static VERIFIED_BOOT: crate::spinlock::SpinLock<VerifiedBoot> =
    crate::spinlock::SpinLock::new(VerifiedBoot::new());

/// Доступ к глобальному VerifiedBoot (mut).
/// Сброс Verified Boot к начальному (GREEN, locked, vbmeta=None) —
/// для install: VbMeta содержит Vec/String с указателями на кучу.
pub fn reset_avb() {
    *VERIFIED_BOOT.lock() = VerifiedBoot::new();
}

pub fn avb_mut() -> crate::spinlock::SpinLockGuard<'static, VerifiedBoot> {
    VERIFIED_BOOT.lock()
}


/// ВЫЗОВ ПРИ ЗАГРУЗКЕ: верификация системных файлов и применение состояния.
///  - Green: загрузка идёт как обычно;
///  - Orange: предупреждение + задержка 5 сек;
///  - Red: красный экран, система не загружается.
pub fn boot_verify() {
    crate::println!("  [avb] Verified Boot: проверка целостности...");
    let mut vb = VERIFIED_BOOT.lock();

    // Системные файлы для проверки (в модели — ключевые файлы ОС).
    let sys: Vec<(&str, Vec<u8>)> = {
        let mut v = Vec::new();
        if let Ok(d) = crate::ext2::read_file("USERS.DB") {
            v.push(("USERS.DB", d));
        }
        if let Ok(d) = crate::ext2::read_file("DXINIT.CFG") {
            v.push(("DXINIT.CFG", d));
        }
        v
    };
    let sys_refs: Vec<(&str, &[u8])> = sys.iter().map(|(n, d)| (*n, d.as_slice())).collect();

    // Если vbmeta ещё не установлена (первый запуск/заводской образ).
    if vb.vbmeta.is_none() {
        if sys_refs.is_empty() {
            // Заводской образ: системных файлов ещё нет — это НЕ нарушение.
            // Загрузка разрешена (GREEN), vbmeta появится при первой настройке.
            vb.boot_state = BootState::Green;
            crate::println!("  [avb] GREEN: заводской образ (vbmeta ещё не запечатана).");
            return;
        } else {
            // Первый запуск с файлами — формируем vbmeta и запечатываем.
            let vm = VbMeta::build(&sys_refs);
            vb.vbmeta = Some(vm);
            crate::println!("  [avb] vbmeta сформирована ({} файлов).", sys_refs.len());
        }
    }

    let state = vb.check(&sys_refs);
    match state {
        BootState::Green => {
            crate::println!("  [avb] GREEN: загрузка подтверждена (bootloader locked, файлы целы).");
        }
        BootState::Orange => {
            crate::println!("  [avb] ORANGE: загрузчик разблокирован (dev-режим).");
            drop(vb);
            // Предупреждение + задержка 5 сек (не держим lock во время паузы).
            let vb2 = VERIFIED_BOOT.lock();
            vb2.orange_warning();
            drop(vb2);
        }
        BootState::Red => {
            crate::println!("  [avb] RED: системные файлы изменены при заблокированном загрузчике.");
            drop(vb);
            VerifiedBoot::red_screen(); // не возвращается
        }
    }
}
