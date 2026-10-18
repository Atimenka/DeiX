//! VFS — единая файловая система DeiX поверх драйверов разделов.
//!
//! Все подсистемы ядра (CLI, pkg, MEX, ELF-загрузчик, GUI, Dinit, Vault)
//! ходят за файлами сюда, а не в `ext2::` / `erofs::` напрямую. Это даёт
//! одно место для проверки прав, одно место для монтирования и один
//! набор кодов ошибок.
//!
//! ## Точки монтирования
//!
//! ```text
//! VFS
//! ├── /system    → EROFS, только чтение (P1)
//! └── /userdata  → EXT2, чтение/запись  (P2)
//! ```
//!
//! Раздел `/system` неизменяем, поэтому записывающие операции на нём
//! возвращают [`VfsError::ReadOnly`], а не обращаются к диску.
//!
//! ## Чтение /system
//!
//! EROFS-раздел с диска целиком не читается: `erofs_disk` отдаёт
//! читателю только запрошенные байты, а метаданные (суперблок, иноды,
//! каталоги) держит в блочном LRU-кеше до 2 МиБ. Кеш сбрасывается только
//! при явном перемонтировании.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Точка монтирования /system.
const MOUNT_SYSTEM: &str = "/system";
/// Точка монтирования /userdata.
const MOUNT_USERDATA: &str = "/userdata";

/// Ошибки VFS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VfsError {
    /// Путь не начинается ни с одной точки монтирования.
    NotMounted { path: String },
    /// Файл или каталог не найден.
    NotFound { path: String },
    /// Раздел смонтирован только для чтения.
    ReadOnly { path: String },
    /// Ожидался каталог, найден файл (или наоборот).
    NotDir { path: String },
    /// Ошибка драйвера раздела.
    Io { path: String, cause: String },
}

impl VfsError {
    pub fn message(&self) -> String {
        match self {
            VfsError::NotMounted { path } => format!("не смонтировано: {}", path),
            VfsError::NotFound { path } => format!("не найдено: {}", path),
            VfsError::ReadOnly { path } => format!("только чтение: {}", path),
            VfsError::NotDir { path } => format!("не каталог: {}", path),
            VfsError::Io { path, cause } => format!("{}: {}", path, cause),
        }
    }
}

/// Запись каталога в терминах VFS.
#[derive(Debug, Clone)]
pub struct DirEnt {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

/// Метаданные объекта.
#[derive(Debug, Clone)]
pub struct Stat {
    pub is_dir: bool,
    pub size: u64,
    /// `true`, если объект лежит на разделе только для чтения.
    pub read_only: bool,
}

// ==================== Доступ к образу /system ====================

/// Раздел /system как источник байт для читателя EROFS. Ничего не
/// читает с диска и не держит в памяти: данные подтягивает `erofs_disk`.
fn system_image() -> Result<crate::erofs_disk::DiskImage, VfsError> {
    let layout = crate::partition_map::lookup_layout(MOUNT_SYSTEM)
        .ok_or_else(|| VfsError::NotMounted { path: String::from(MOUNT_SYSTEM) })?;
    Ok(crate::erofs_disk::DiskImage::new(layout.start_lba, layout.sectors))
}

// ==================== Тип ФС раздела /system ====================

/// Какая файловая система на /system. Штатно это EROFS; утилита `sysfs
/// convert` (dev-режим) переформатирует раздел в ext2 — после этого vfs
/// обязан читать его драйвером ext2. Определяется по магии суперблока
/// (обе сигнатуры лежат в блоке на смещении 1024 от начала раздела:
/// ext2 — 0xEF53 на +56, EROFS — 0xE0F5E1E2 на +0).
/// 0 = ещё не определяли, 1 = EROFS, 2 = ext2.
static SYSTEM_FS: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

fn system_is_ext2() -> bool {
    use core::sync::atomic::Ordering;
    match SYSTEM_FS.load(Ordering::Relaxed) {
        1 => false,
        2 => true,
        _ => {
            let is = crate::ext2::is_formatted_vol(crate::ext2::VolId::System);
            SYSTEM_FS.store(if is { 2 } else { 1 }, Ordering::Relaxed);
            is
        }
    }
}

/// Разрешена ли запись на /system: только после конверсии в ext2 и
/// только в dev-режиме.
fn system_writable() -> bool {
    system_is_ext2() && crate::devmode::sudo_allowed()
}

/// Перемонтирует /system после конверсии EROFS→ext2: сбрасывает
/// определение типа ФС, кеш геометрии ext2 и блочный кеш EROFS.
pub fn remount_system() {
    use core::sync::atomic::Ordering;
    SYSTEM_FS.store(0, Ordering::Relaxed);
    crate::ext2::invalidate_cache(crate::ext2::VolId::System);
    crate::erofs_disk::invalidate();
}

fn system_ext2_err(path: &str, e: crate::ext2::Ext2Error) -> VfsError {
    match e {
        crate::ext2::Ext2Error::FileNotFound => VfsError::NotFound { path: String::from(path) },
        other => VfsError::Io { path: String::from(path), cause: ext2_cause(other) },
    }
}
// ==================== Разбор путей ====================

/// Разбирает абсолютный путь на точку монтирования и остаток пути
/// внутри раздела.
fn split_mount(path: &str) -> Result<(&'static str, String), VfsError> {
    let trimmed = path.trim();
    if trimmed == MOUNT_SYSTEM || trimmed.starts_with("/system/") {
        let rest = trimmed.strip_prefix(MOUNT_SYSTEM).unwrap_or("");
        return Ok((MOUNT_SYSTEM, normalize(rest)));
    }
    if trimmed == MOUNT_USERDATA || trimmed.starts_with("/userdata/") {
        let rest = trimmed.strip_prefix(MOUNT_USERDATA).unwrap_or("");
        return Ok((MOUNT_USERDATA, normalize(rest)));
    }
    Err(VfsError::NotMounted { path: String::from(trimmed) })
}

/// Приводит остаток пути к виду, который понимают драйверы разделов:
/// без ведущих и повторяющихся слэшей. Пустая строка означает корень.
fn normalize(rest: &str) -> String {
    let mut out = String::new();
    for part in rest.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(part);
    }
    out
}
// ==================== Операции ====================

/// Читает файл целиком.
pub fn read_file(path: &str) -> Result<Vec<u8>, VfsError> {
    // TRACE отбрасывается ещё до форматирования строки, если
    // диагностический режим выключен — обычный путь чтения не платит
    // за журналирование ничем, кроме одной атомарной загрузки.
    // Обращения к /userdata/log не трассируются: сброс журнала на диск
    // сам ходит через VFS, и трассировка этих обращений порождала бы
    // новые события при каждой записи журнала — без ограничения.
    if crate::diag::passes(crate::diag::Severity::Trace) && !path.starts_with("/userdata/log") {
        crate::diag::trace(crate::diag::NONE, &format!("VFS: read {}", path));
    }
    let (mount, rest) = split_mount(path)?;
    match mount {
        MOUNT_SYSTEM => {
            if system_is_ext2() {
                let key = userdata_key(&rest);
                return crate::ext2::read_file_path_vol(crate::ext2::VolId::System, &key)
                    .map_err(|e| system_ext2_err(path, e));
            }
            let img = system_image()?;
            crate::erofs::read_file(&img, &rest).map_err(|e| match e {
                crate::erofs::ErofsError::NotFound => VfsError::NotFound { path: String::from(path) },
                other => VfsError::Io { path: String::from(path), cause: other.message() },
            })
        }
        MOUNT_USERDATA => {
            let key = userdata_key(&rest);
            crate::ext2::read_file_path(&key).map_err(|e| match e {
                crate::ext2::Ext2Error::FileNotFound => {
                    VfsError::NotFound { path: String::from(path) }
                }
                other => VfsError::Io {
                    path: String::from(path),
                    cause: ext2_cause(other),
                },
            })
        }
        _ => Err(VfsError::NotMounted { path: String::from(path) }),
    }
}

/// Пишет файл. На `/system` всегда отказывает.
pub fn write_file(path: &str, data: &[u8]) -> Result<(), VfsError> {
    if crate::diag::passes(crate::diag::Severity::Trace) && !path.starts_with("/userdata/log") {
        crate::diag::trace(
            crate::diag::NONE,
            &format!("VFS: write {} ({} Б)", path, data.len()),
        );
    }
    let (mount, rest) = split_mount(path)?;
    match mount {
        MOUNT_SYSTEM => {
            if !system_writable() {
                return Err(VfsError::ReadOnly { path: String::from(path) });
            }
            let key = userdata_key(&rest);
            crate::ext2::write_file_path_vol(crate::ext2::VolId::System, &key, data)
                .map_err(|e| system_ext2_err(path, e))
        }
        MOUNT_USERDATA => {
            let key = userdata_key(&rest);
            crate::ext2::write_file_path(&key, data).map_err(|e| VfsError::Io {
                path: String::from(path),
                cause: ext2_cause(e),
            })
        }
        _ => Err(VfsError::NotMounted { path: String::from(path) }),
    }
}

/// Создаёт каталог, включая недостающие промежуточные.
pub fn mkdir(path: &str) -> Result<(), VfsError> {
    let (mount, rest) = split_mount(path)?;
    match mount {
        MOUNT_SYSTEM => {
            if !system_writable() {
                return Err(VfsError::ReadOnly { path: String::from(path) });
            }
            let key = userdata_key(&rest);
            crate::ext2::mkdir_p_vol(crate::ext2::VolId::System, &key)
                .map(|_| ())
                .map_err(|e| system_ext2_err(path, e))
        }
        MOUNT_USERDATA => {
            let key = userdata_key(&rest);
            crate::ext2::mkdir_p(&key).map(|_| ()).map_err(|e| VfsError::Io {
                path: String::from(path),
                cause: ext2_cause(e),
            })
        }
        _ => Err(VfsError::NotMounted { path: String::from(path) }),
    }
}

/// Удаляет файл.
pub fn remove(path: &str) -> Result<(), VfsError> {
    let (mount, rest) = split_mount(path)?;
    match mount {
        MOUNT_SYSTEM => {
            if !system_writable() {
                return Err(VfsError::ReadOnly { path: String::from(path) });
            }
            let key = userdata_key(&rest);
            crate::ext2::delete_file_path_vol(crate::ext2::VolId::System, &key)
                .map_err(|e| system_ext2_err(path, e))
        }
        MOUNT_USERDATA => {
            let key = userdata_key(&rest);
            crate::ext2::delete_file_path(&key).map_err(|e| VfsError::Io {
                path: String::from(path),
                cause: ext2_cause(e),
            })
        }
        _ => Err(VfsError::NotMounted { path: String::from(path) }),
    }
}

/// Метаданные объекта.
pub fn stat(path: &str) -> Result<Stat, VfsError> {
    let (mount, rest) = split_mount(path)?;
    match mount {
        MOUNT_SYSTEM => {
            if system_is_ext2() {
                let read_only = !system_writable();
                if rest.is_empty() {
                    return Ok(Stat { is_dir: true, size: 0, read_only });
                }
                let (parent, leaf) = split_parent(&rest);
                let key = userdata_key(&parent);
                let entries = crate::ext2::list_dir_path_vol(crate::ext2::VolId::System, &key)
                    .map_err(|e| system_ext2_err(path, e))?;
                return entries
                    .into_iter()
                    .find(|e| e.name == leaf)
                    .map(|e| Stat { is_dir: e.is_directory, size: e.size as u64, read_only })
                    .ok_or_else(|| VfsError::NotFound { path: String::from(path) });
            }
            let img = system_image()?;
            // Корень раздела: у EROFS-корня нет записи в родителе,
            // поэтому проверяем только что образ вообще разбирается.
            if rest.is_empty() {
                return crate::erofs::parse_superblock(&img)
                    .map(|_| Stat {
                        is_dir: true,
                        size: 0,
                        read_only: true,
                    })
                    .map_err(|e| VfsError::Io {
                        path: String::from(path),
                        cause: e.message(),
                    });
            }
            match crate::erofs::stat(&img, &rest) {
                Ok(ino) => Ok(Stat {
                    is_dir: ino.is_dir(),
                    size: ino.size,
                    read_only: true,
                }),
                Err(crate::erofs::ErofsError::NotFound) => {
                    Err(VfsError::NotFound { path: String::from(path) })
                }
                Err(other) => Err(VfsError::Io {
                    path: String::from(path),
                    cause: other.message(),
                }),
            }
        }
        MOUNT_USERDATA => {
            // Корень тома — сам каталог, у него нет записи в родителе.
            if rest.is_empty() {
                return Ok(Stat {
                    is_dir: true,
                    size: 0,
                    read_only: false,
                });
            }
            // EXT2-драйвер не даёт stat по пути: ищем запись в родителе.
            let (parent, leaf) = split_parent(&rest);
            let key = userdata_key(&parent);
            let entries = crate::ext2::list_dir_path(&key).map_err(|e| VfsError::Io {
                path: String::from(path),
                cause: ext2_cause(e),
            })?;
            entries
                .into_iter()
                .find(|e| e.name == leaf)
                .map(|e| Stat {
                    is_dir: e.is_directory,
                    size: e.size as u64,
                    read_only: false,
                })
                .ok_or_else(|| VfsError::NotFound { path: String::from(path) })
        }
        _ => Err(VfsError::NotMounted { path: String::from(path) }),
    }
}

/// Содержимое каталога.
pub fn readdir(path: &str) -> Result<Vec<DirEnt>, VfsError> {
    let (mount, rest) = split_mount(path)?;
    // Корень тома — всегда каталог; для остальных путей сначала
    // проверяем тип, чтобы листинг файла давал внятную ошибку.
    if !rest.is_empty() && !stat(path)?.is_dir {
        return Err(VfsError::NotDir { path: String::from(path) });
    }
    match mount {
        MOUNT_SYSTEM => {
            if system_is_ext2() {
                let key = userdata_key(&rest);
                let entries = crate::ext2::list_dir_path_vol(crate::ext2::VolId::System, &key)
                    .map_err(|e| system_ext2_err(path, e))?;
                return Ok(entries
                    .into_iter()
                    .map(|e| DirEnt { name: e.name, is_dir: e.is_directory, size: e.size as u64 })
                    .collect());
            }
            let img = system_image()?;
            let listing = crate::erofs::list_dir(&img, &rest).map_err(|e| match e {
                crate::erofs::ErofsError::NotFound => VfsError::NotFound { path: String::from(path) },
                other => VfsError::Io { path: String::from(path), cause: other.message() },
            })?;
            Ok(listing
                .into_iter()
                .map(|e| DirEnt { name: e.name, is_dir: e.is_dir, size: e.size })
                .collect())
        }
        MOUNT_USERDATA => {
            let key = userdata_key(&rest);
            let entries = crate::ext2::list_dir_path(&key).map_err(|e| VfsError::Io {
                path: String::from(path),
                cause: ext2_cause(e),
            })?;
            Ok(entries
                .into_iter()
                .map(|e| DirEnt {
                    name: e.name,
                    is_dir: e.is_directory,
                    size: e.size as u64,
                })
                .collect())
        }
        _ => Err(VfsError::NotMounted { path: String::from(path) }),
    }
}

/// Текущее состояние точек монтирования — для отчётов и `diagnostics`.
///
/// Не читает `/system` с диска и не берёт блокировки безусловно: функция
/// вызывается в том числе из отчёта об отказе, где зависание на блокировке
/// недопустимо, а чтение 8 МиБ с диска бессмысленно.
pub fn describe_mounts() -> Vec<String> {
    let mut out = Vec::new();

    let system = if SYSTEM_FS.load(core::sync::atomic::Ordering::Relaxed) == 2 {
        let mode = if crate::devmode::sudo_allowed() { "rw (dev)" } else { "ro" };
        format!("{} {} ext2 (converted)", MOUNT_SYSTEM, mode)
    } else {
        format!("{} ro erofs ({})", MOUNT_SYSTEM, crate::erofs_disk::describe())
    };
    out.push(system);

    let userdata = if crate::ext2::is_formatted() {
        format!("{} rw ext2", MOUNT_USERDATA)
    } else {
        format!("{} НЕ ОТФОРМАТИРОВАН", MOUNT_USERDATA)
    };
    out.push(userdata);

    out
}

/// Проверяет существование пути.
pub fn exists(path: &str) -> bool {
    stat(path).is_ok()
}

// ==================== Вспомогательное ====================

/// Драйвер EXT2 работает с путями внутри тома. Корень тома — пустая
/// строка у VFS, но драйвер ожидает имя без ведущего слэша.
fn userdata_key(rest: &str) -> String {
    if rest.is_empty() {
        String::from("/")
    } else {
        String::from(rest)
    }
}

/// Разделяет путь на родителя и последний компонент.
fn split_parent(rest: &str) -> (String, String) {
    match rest.rfind('/') {
        Some(idx) => (String::from(&rest[..idx]), String::from(&rest[idx + 1..])),
        None => (String::new(), String::from(rest)),
    }
}

fn ext2_cause(e: crate::ext2::Ext2Error) -> String {
    use crate::ext2::Ext2Error as E;
    // Инфраструктурные сбои EXT2 регистрируются в журнале; «не найдено» и
    // прочие ожидаемые исходы операций — нет, иначе каждый промах поиска
    // файла станет строкой в error.log.
    match e {
        E::DiskError => {
            // Ошибка диска почти всегда вторична к только что
            // зарегистрированному отказу ATA — связываем записи в цепочку
            // «первопричина → вторичный сбой».
            let now = crate::timer::uptime_ms();
            let cause =
                crate::diag::ring::find_recent_error(crate::diag::Subsystem::Disk, 2_000, now);
            let code = crate::diag::ErrorCode::new(crate::diag::Subsystem::Ext2, 6);
            match cause {
                Some(id) => {
                    crate::diag::caused_by(id, code, "EXT2: ошибка диска при обращении к тому");
                }
                None => {
                    crate::diag::error(code, "EXT2: ошибка диска при обращении к тому");
                }
            }
            String::from("ошибка диска")
        }
        E::NotFormatted => {
            crate::diag::warn(
                crate::diag::ErrorCode::new(crate::diag::Subsystem::Ext2, 1),
                "EXT2: том не отформатирован",
            );
            String::from("раздел не отформатирован")
        }
        E::NoSpace => {
            crate::diag::error(
                crate::diag::ErrorCode::new(crate::diag::Subsystem::Ext2, 9),
                "EXT2: на /userdata закончились свободные блоки",
            );
            String::from("нет места")
        }
        E::FileNotFound => String::from("не найдено"),
        E::InvalidName => String::from("недопустимое имя"),
        E::FileTooLarge => String::from("файл слишком большой"),
        E::DirectoryFull => String::from("каталог полон"),
    }
}
