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
//! ## Кеш образа /system
//!
//! EROFS-раздел читается с диска целиком при первом обращении и дальше
//! держится в куче: повторное чтение каждого файла стоило бы полного
//! прохода по 8704 секторам. Кеш инвалидируется только при явном
//! перемонтировании.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::spinlock::SpinLock;

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
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    /// `true`, если объект лежит на разделе только для чтения.
    pub read_only: bool,
}

// ==================== Кеш образа /system ====================

/// Прочитанный образ EROFS-раздела /system.
static SYSTEM_IMAGE: SpinLock<Option<Vec<u8>>> = SpinLock::new(None);

fn system_image() -> Result<Vec<u8>, VfsError> {
    {
        let guard = SYSTEM_IMAGE.lock();
        if let Some(img) = guard.as_ref() {
            return Ok(img.clone());
        }
    }

    let layout = crate::partition_map::lookup_layout(MOUNT_SYSTEM)
        .ok_or_else(|| VfsError::NotMounted { path: String::from(MOUNT_SYSTEM) })?;
    let img = crate::bootchain::read_partition_image(layout).map_err(|e| VfsError::Io {
        path: String::from(MOUNT_SYSTEM),
        cause: e,
    })?;

    let mut guard = SYSTEM_IMAGE.lock();
    *guard = Some(img.clone());
    Ok(img)
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
    let (mount, rest) = split_mount(path)?;
    match mount {
        MOUNT_SYSTEM => {
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
    let (mount, rest) = split_mount(path)?;
    match mount {
        MOUNT_SYSTEM => Err(VfsError::ReadOnly { path: String::from(path) }),
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
        MOUNT_SYSTEM => Err(VfsError::ReadOnly { path: String::from(path) }),
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
        MOUNT_SYSTEM => Err(VfsError::ReadOnly { path: String::from(path) }),
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
            let img = system_image()?;
            // Корень раздела: у EROFS-корня нет записи в родителе,
            // поэтому проверяем только что образ вообще разбирается.
            if rest.is_empty() {
                return crate::erofs::parse_superblock(&img)
                    .map(|_| Stat {
                        path: String::from(MOUNT_SYSTEM),
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
                    path: String::from(path),
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
                    path: String::from(MOUNT_USERDATA),
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
                    path: String::from(path),
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
    match mount {
        MOUNT_SYSTEM => {
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
    match e {
        E::DiskError => String::from("ошибка диска"),
        E::NotFormatted => String::from("раздел не отформатирован"),
        E::FileNotFound => String::from("не найдено"),
        E::NoSpace => String::from("нет места"),
        E::InvalidName => String::from("недопустимое имя"),
        E::FileTooLarge => String::from("файл слишком большой"),
        E::DirectoryFull => String::from("каталог полон"),
    }
}
